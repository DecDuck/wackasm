package dev.wackasm.oc.ae2;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import appeng.api.config.Actionable;
import appeng.api.crafting.IPatternDetails;
import appeng.api.crafting.PatternDetailsHelper;
import appeng.api.networking.GridFlags;
import appeng.api.networking.GridHelper;
import appeng.api.networking.IGridNode;
import appeng.api.networking.IGridNodeListener;
import appeng.api.networking.IInWorldGridNodeHost;
import appeng.api.networking.IManagedGridNode;
import appeng.api.networking.crafting.ICraftingProvider;
import appeng.api.networking.security.IActionHost;
import appeng.api.networking.security.IActionSource;
import appeng.api.stacks.AEFluidKey;
import appeng.api.stacks.AEItemKey;
import appeng.api.stacks.AEKey;
import appeng.api.stacks.GenericStack;
import appeng.api.stacks.KeyCounter;
import appeng.api.storage.StorageHelper;
import appeng.crafting.pattern.AEProcessingPattern;
import li.cil.oc.api.Network;
import li.cil.oc.api.machine.Arguments;
import li.cil.oc.api.machine.Callback;
import li.cil.oc.api.machine.Context;
import li.cil.oc.api.machine.Machine;
import li.cil.oc.api.network.Node;
import li.cil.oc.api.network.Visibility;
import li.cil.oc.api.prefab.BlockEntityEnvironment;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.HolderLookup;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.Tag;
import net.minecraft.world.Containers;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.state.BlockState;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.fluids.FluidStack;
import net.neoforged.neoforge.fluids.capability.IFluidHandler;
import net.neoforged.neoforge.items.IItemHandler;
import net.neoforged.neoforge.items.ItemHandlerHelper;

/**
 * A pattern provider whose patterns and processing a computer controls. It's
 * an AE2 grid node and an OpenComputers component ({@value #COMPONENT}) at
 * once.
 *
 * <ul>
 *   <li>The computer declares processing patterns (inputs to outputs) with
 *       {@code setPatterns} or {@code addPattern}. They're encoded as ordinary
 *       AE2 processing patterns, so terminals and the crafting planner treat
 *       them like any others.</li>
 *   <li>When a crafting job uses one, AE2 hands this block the inputs. They're
 *       held here as a <em>job</em>, and the computer gets a
 *       {@code pattern_request} signal. {@code getJobs} lists what's waiting,
 *       so a program that missed the signal (or just booted) can catch up.</li>
 *   <li>The computer moves a job's inputs into an adjacent inventory with
 *       {@code pushInputs}, does whatever processing it likes, and the
 *       results have to be imported into the ME network like any processing
 *       pattern's: through an ME Interface, an import bus, or this block,
 *       which forwards items inserted into it to the network. AE2's crafting
 *       CPUs claim them as they arrive. Nothing here creates items.</li>
 * </ul>
 *
 * <p>Callbacks are synchronized with the server tick (none are direct), so all
 * state is only touched on the server thread.
 */
public final class PatternProviderBlockEntity extends BlockEntityEnvironment
        implements IInWorldGridNodeHost, ICraftingProvider, IActionHost {

    static final String COMPONENT = "pattern_provider";
    /** Jobs held at once. Past this, AE2 waits before sending more. */
    static final int MAX_JOBS = 64;
    static final int MAX_PATTERNS = 256;

    private static final IGridNodeListener<PatternProviderBlockEntity> LISTENER = new IGridNodeListener<>() {
        @Override
        public void onSaveChanges(PatternProviderBlockEntity host, IGridNode node) {
            host.setChanged();
        }
    };

    private final IManagedGridNode mainNode = GridHelper.createManagedNode(this, LISTENER)
            .setFlags(GridFlags.REQUIRE_CHANNEL)
            .setIdlePowerUsage(1.0)
            .setInWorldNode(true)
            .setTagName("proxy")
            .setVisualRepresentation(Ae2Integration.PATTERN_PROVIDER_ITEM.get())
            .addService(ICraftingProvider.class, this);

    private final IActionSource actionSource = IActionSource.ofMachine(this);

    /** The patterns, in the order the computer gave them (Lua indices are 1-based). */
    private final List<Pattern> patterns = new ArrayList<>();
    /** The jobs waiting on the computer, oldest first. */
    private final Map<Long, Job> jobs = new LinkedHashMap<>();
    private long nextJobId = 1;
    /** Whether a running computer can reach this block; refreshed every second. */
    private boolean computerAttached;
    private int ticks;

    private final IItemHandler returnInventory = new ReturnInventory();

    /** A processing pattern, and the AE2 pattern it was encoded as (null until there is a level). */
    private static final class Pattern {
        final List<GenericStack> inputs;
        final List<GenericStack> outputs;
        IPatternDetails details;

        Pattern(List<GenericStack> inputs, List<GenericStack> outputs) {
            this.inputs = inputs;
            this.outputs = outputs;
        }
    }

    /**
     * The inputs AE2 handed over for one use of a pattern, minus what has been
     * pushed out so far, and the outputs AE2 expects back.
     */
    private record Job(long id, int pattern, Map<AEKey, Long> inputs, List<GenericStack> outputs) {}

    public PatternProviderBlockEntity(BlockPos pos, BlockState state) {
        super(Ae2Integration.PATTERN_PROVIDER_BLOCK_ENTITY.get(), pos, state);
        node = Network.newNode(this, Visibility.Network).withComponent(COMPONENT, Visibility.Network).create();
    }

    // ------------------------------------------------------------------ lifecycle

    @Override
    public void onLoad() {
        super.onLoad();
        if (level != null && !level.isClientSide) {
            GridHelper.onFirstTick(this, be -> be.mainNode.create(be.level, be.worldPosition));
        }
    }

    @Override
    public void setRemoved() {
        super.setRemoved();
        mainNode.destroy();
    }

    @Override
    public void onChunkUnloaded() {
        super.onChunkUnloaded();
        mainNode.destroy();
    }

    void serverTick() {
        updateEntity(); // joins the OpenComputers network once
        if (ticks++ % 20 == 0) computerAttached = findComputer();
    }

    void setOwner(Player player) {
        mainNode.setOwningPlayer(player);
    }

    /** Drops the items held for jobs; fluids are lost. Called when the block is broken. */
    void dropContents() {
        if (level == null) return;
        for (Job job : jobs.values()) {
            for (Map.Entry<AEKey, Long> e : job.inputs.entrySet()) {
                if (!(e.getKey() instanceof AEItemKey item)) continue;
                long left = e.getValue();
                int max = item.getMaxStackSize();
                while (left > 0) {
                    int n = (int) Math.min(left, max);
                    Containers.dropItemStack(level, worldPosition.getX(), worldPosition.getY(), worldPosition.getZ(),
                            item.toStack(n));
                    left -= n;
                }
            }
        }
        jobs.clear();
    }

    private boolean findComputer() {
        if (node == null || node.network() == null) return false;
        for (Node other : node.reachableNodes()) {
            if (other.host() instanceof Machine machine && machine.isRunning()) return true;
        }
        return false;
    }

    IItemHandler returnInventory() {
        return returnInventory;
    }

    // ------------------------------------------------------------------ AE2

    @Override
    public IGridNode getGridNode(Direction dir) {
        return mainNode.getNode();
    }

    @Override
    public IGridNode getActionableNode() {
        return mainNode.getNode();
    }

    @Override
    public List<IPatternDetails> getAvailablePatterns() {
        List<IPatternDetails> available = new ArrayList<>(patterns.size());
        for (Pattern pattern : patterns) {
            IPatternDetails details = details(pattern);
            if (details != null) available.add(details);
        }
        return available;
    }

    @Override
    public boolean pushPattern(IPatternDetails details, KeyCounter[] inputs) {
        if (!mainNode.isActive() || isBusy()) return false;
        int index = indexOf(details);
        if (index < 0) return false;

        Map<AEKey, Long> held = new LinkedHashMap<>();
        for (KeyCounter counter : inputs) {
            for (var e : counter) held.merge(e.getKey(), e.getLongValue(), Long::sum);
        }
        long id = nextJobId++;
        jobs.put(id, new Job(id, index, held, List.copyOf(details.getOutputs())));
        setChanged();
        // A hint: the job is held until the computer deals with it, signal or not.
        if (node.network() != null) node.sendToReachable("computer.signal", "pattern_request", node.address(), id, index + 1);
        return true;
    }

    @Override
    public boolean isBusy() {
        // With no computer to take them, inputs would only pile up here.
        return !computerAttached || jobs.size() >= MAX_JOBS;
    }

    private IPatternDetails details(Pattern pattern) {
        if (pattern.details == null && level != null) {
            try {
                ItemStack encoded = PatternDetailsHelper.encodeProcessingPattern(pattern.inputs, pattern.outputs);
                pattern.details = PatternDetailsHelper.decodePattern(encoded, level);
            } catch (RuntimeException e) {
                // Checked when it was added, so only content removed since (a mod uninstalled) gets here.
                Ae2Integration.log.warn("Pattern provider at {} has an unusable pattern: {}", worldPosition, e.toString());
            }
        }
        return pattern.details;
    }

    private int indexOf(IPatternDetails details) {
        for (int i = 0; i < patterns.size(); i++) {
            if (details.equals(details(patterns.get(i)))) return i;
        }
        return -1;
    }

    private void patternsChanged() {
        setChanged();
        ICraftingProvider.requestUpdate(mainNode);
    }

    /** Inserts into the network, drawing power as AE2 does for its own imports. Returns how much went in. */
    private long insertIntoNetwork(AEKey key, long amount, Actionable mode) {
        var grid = mainNode.getGrid();
        if (grid == null || !mainNode.isActive()) return 0;
        return StorageHelper.poweredInsert(grid.getEnergyService(), grid.getStorageService().getInventory(),
                key, amount, actionSource, mode);
    }

    // ------------------------------------------------------------------ OpenComputers

    @Callback(doc = "function(patterns:table):number -- Replaces every pattern. Each is {inputs={stack...}, "
            + "outputs={stack...}}, where a stack is {name=string, count=number[, fluid=boolean]} (or {key=string, "
            + "count=number} as returned by getPatterns). Returns the number of patterns.")
    public Object[] setPatterns(Context context, Arguments args) {
        List<Object> list = Stacks.list(args.checkAny(0), "patterns");
        if (list.size() > MAX_PATTERNS) throw new IllegalArgumentException("at most " + MAX_PATTERNS + " patterns");
        List<Pattern> parsed = new ArrayList<>(list.size());
        for (int i = 0; i < list.size(); i++) {
            if (!(list.get(i) instanceof Map<?, ?> table)) {
                throw new IllegalArgumentException("pattern " + (i + 1) + " must be a table");
            }
            parsed.add(parse(table.get("inputs"), table.get("outputs"), parsed, "pattern " + (i + 1) + ": "));
        }
        patterns.clear();
        patterns.addAll(parsed);
        patternsChanged();
        return new Object[] {patterns.size()};
    }

    @Callback(doc = "function(inputs:table, outputs:table):number -- Adds a pattern (see setPatterns). Returns its index.")
    public Object[] addPattern(Context context, Arguments args) {
        if (patterns.size() >= MAX_PATTERNS) throw new IllegalArgumentException("at most " + MAX_PATTERNS + " patterns");
        patterns.add(parse(args.checkAny(0), args.checkAny(1), patterns, ""));
        patternsChanged();
        return new Object[] {patterns.size()};
    }

    @Callback(doc = "function(index:number):boolean -- Removes a pattern; later ones move down one index. "
            + "Jobs already handed over are kept.")
    public Object[] removePattern(Context context, Arguments args) {
        int index = args.checkInteger(0) - 1;
        if (index < 0 || index >= patterns.size()) return new Object[] {false};
        patterns.remove(index);
        patternsChanged();
        return new Object[] {true};
    }

    @Callback(doc = "function():table -- Every pattern, as {inputs={stack...}, outputs={stack...}}.")
    public Object[] getPatterns(Context context, Arguments args) {
        HolderLookup.Provider registries = registries();
        Object[] out = new Object[patterns.size()];
        for (int i = 0; i < out.length; i++) {
            Pattern p = patterns.get(i);
            Map<String, Object> table = new LinkedHashMap<>();
            table.put("inputs", Stacks.describe(p.inputs, registries));
            table.put("outputs", Stacks.describe(p.outputs, registries));
            out[i] = table;
        }
        return new Object[] {out};
    }

    @Callback(doc = "function():table -- The jobs waiting, oldest first, as {id=number, pattern=number, "
            + "inputs={stack...}, outputs={stack...}}. inputs is what is still held here; outputs is what the "
            + "crafting job expects to be imported into the network.")
    public Object[] getJobs(Context context, Arguments args) {
        return new Object[] {jobs.values().stream().map(this::describe).toArray()};
    }

    @Callback(doc = "function(id:number):table or nil -- One job, as in getJobs; nil once it's done or cancelled.")
    public Object[] getJob(Context context, Arguments args) {
        Job job = jobs.get(args.checkLong(0));
        return new Object[] {job == null ? null : describe(job)};
    }

    @Callback(doc = "function(id:number, side:number):boolean -- Moves a job's inputs into the inventory (or fluid "
            + "tank) on that side of this block. Returns true once everything is out, which ends the job; false "
            + "if some didn't fit, which stays held for another try.")
    public Object[] pushInputs(Context context, Arguments args) {
        Job job = jobs.get(args.checkLong(0));
        if (job == null) throw new IllegalArgumentException("no such job");
        int side = args.checkInteger(1);
        if (side < 0 || side > 5) throw new IllegalArgumentException("side must be 0 to 5");
        Direction dir = Direction.from3DDataValue(side);
        BlockPos target = worldPosition.relative(dir);
        IItemHandler items = level.getCapability(Capabilities.ItemHandler.BLOCK, target, dir.getOpposite());
        IFluidHandler fluids = level.getCapability(Capabilities.FluidHandler.BLOCK, target, dir.getOpposite());

        Iterator<Map.Entry<AEKey, Long>> it = job.inputs.entrySet().iterator();
        while (it.hasNext()) {
            Map.Entry<AEKey, Long> e = it.next();
            long left = e.getValue();
            if (e.getKey() instanceof AEItemKey item && items != null) {
                left = pushItems(item, left, items);
            } else if (e.getKey() instanceof AEFluidKey fluid && fluids != null) {
                int n = (int) Math.min(left, Integer.MAX_VALUE);
                left -= fluids.fill(fluid.toStack(n), IFluidHandler.FluidAction.EXECUTE);
            }
            if (left <= 0) it.remove();
            else e.setValue(left);
        }
        setChanged();
        if (!job.inputs.isEmpty()) return new Object[] {false};
        jobs.remove(job.id);
        return new Object[] {true};
    }

    @Callback(doc = "function(id:number):boolean -- Gives up on a job, returning its held inputs to the network. "
            + "Returns true if they all went back (ending the job), false if some didn't fit. The crafting job in "
            + "AE2 keeps waiting for its outputs; cancel it there.")
    public Object[] cancelJob(Context context, Arguments args) {
        Job job = jobs.get(args.checkLong(0));
        if (job == null) throw new IllegalArgumentException("no such job");
        Iterator<Map.Entry<AEKey, Long>> it = job.inputs.entrySet().iterator();
        while (it.hasNext()) {
            Map.Entry<AEKey, Long> e = it.next();
            long left = e.getValue() - insertIntoNetwork(e.getKey(), e.getValue(), Actionable.MODULATE);
            if (left <= 0) it.remove();
            else e.setValue(left);
        }
        setChanged();
        if (!job.inputs.isEmpty()) return new Object[] {false};
        jobs.remove(job.id);
        return new Object[] {true};
    }

    @Callback(doc = "function():boolean -- Whether this block is connected to a powered ME network with a channel.")
    public Object[] isOnline(Context context, Arguments args) {
        return new Object[] {mainNode.isActive()};
    }

    private Pattern parse(Object inputs, Object outputs, List<Pattern> existing, String where) {
        HolderLookup.Provider registries = registries();
        List<GenericStack> in, out;
        try {
            in = Stacks.parseList(inputs, registries, "inputs");
            out = Stacks.parseList(outputs, registries, "outputs");
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException(where + e.getMessage());
        }
        // AE2's own limits for processing patterns; it rejects a pattern without inputs.
        if (in.isEmpty() || in.size() > AEProcessingPattern.MAX_INPUT_SLOTS) {
            throw new IllegalArgumentException(where + "a pattern needs 1 to " + AEProcessingPattern.MAX_INPUT_SLOTS + " inputs");
        }
        if (out.isEmpty() || out.size() > AEProcessingPattern.MAX_OUTPUT_SLOTS) {
            throw new IllegalArgumentException(where + "a pattern needs 1 to " + AEProcessingPattern.MAX_OUTPUT_SLOTS + " outputs");
        }
        Pattern pattern = new Pattern(in, out);
        if (details(pattern) == null) throw new IllegalArgumentException(where + "AE2 rejected the pattern");
        for (Pattern other : existing) {
            // AE2 tells patterns apart by their encoding, so two identical ones would be one.
            if (pattern.details.equals(details(other))) throw new IllegalArgumentException(where + "duplicate pattern");
        }
        return pattern;
    }

    private Map<String, Object> describe(Job job) {
        HolderLookup.Provider registries = registries();
        Map<String, Object> table = new LinkedHashMap<>();
        table.put("id", job.id);
        table.put("pattern", job.pattern + 1);
        table.put("inputs", job.inputs.entrySet().stream()
                .map(e -> Stacks.describe(e.getKey(), e.getValue(), registries)).toArray());
        table.put("outputs", Stacks.describe(job.outputs, registries));
        return table;
    }

    private static long pushItems(AEItemKey item, long amount, IItemHandler target) {
        int max = item.getMaxStackSize();
        while (amount > 0) {
            int n = (int) Math.min(amount, max);
            ItemStack rest = ItemHandlerHelper.insertItemStacked(target, item.toStack(n), false);
            amount -= n - rest.getCount();
            if (!rest.isEmpty()) break;
        }
        return amount;
    }

    private HolderLookup.Provider registries() {
        return level.registryAccess();
    }

    // ------------------------------------------------------------------ saving

    @Override
    public void saveAdditional(CompoundTag tag, HolderLookup.Provider registries) {
        super.saveAdditional(tag, registries);
        mainNode.saveToNBT(tag);
        ListTag patternList = new ListTag();
        for (Pattern p : patterns) {
            CompoundTag entry = new CompoundTag();
            entry.put("inputs", saveStacks(p.inputs, registries));
            entry.put("outputs", saveStacks(p.outputs, registries));
            patternList.add(entry);
        }
        tag.put("wackasm:patterns", patternList);
        ListTag jobList = new ListTag();
        for (Job job : jobs.values()) {
            CompoundTag entry = new CompoundTag();
            entry.putLong("id", job.id);
            entry.putInt("pattern", job.pattern);
            entry.put("inputs", saveStacks(job.inputs.entrySet().stream()
                    .map(e -> new GenericStack(e.getKey(), e.getValue())).toList(), registries));
            entry.put("outputs", saveStacks(job.outputs, registries));
            jobList.add(entry);
        }
        tag.put("wackasm:jobs", jobList);
        tag.putLong("wackasm:nextJob", nextJobId);
    }

    @Override
    public void loadAdditional(CompoundTag tag, HolderLookup.Provider registries) {
        super.loadAdditional(tag, registries);
        mainNode.loadFromNBT(tag);
        patterns.clear();
        for (Tag t : tag.getList("wackasm:patterns", Tag.TAG_COMPOUND)) {
            CompoundTag entry = (CompoundTag) t;
            patterns.add(new Pattern(loadStacks(entry.getList("inputs", Tag.TAG_COMPOUND), registries),
                    loadStacks(entry.getList("outputs", Tag.TAG_COMPOUND), registries)));
        }
        jobs.clear();
        for (Tag t : tag.getList("wackasm:jobs", Tag.TAG_COMPOUND)) {
            CompoundTag entry = (CompoundTag) t;
            Map<AEKey, Long> held = new LinkedHashMap<>();
            for (GenericStack s : loadStacks(entry.getList("inputs", Tag.TAG_COMPOUND), registries)) {
                held.merge(s.what(), s.amount(), Long::sum);
            }
            long id = entry.getLong("id");
            jobs.put(id, new Job(id, entry.getInt("pattern"), held,
                    loadStacks(entry.getList("outputs", Tag.TAG_COMPOUND), registries)));
        }
        nextJobId = Math.max(1, tag.getLong("wackasm:nextJob"));
    }

    private static ListTag saveStacks(List<GenericStack> stacks, HolderLookup.Provider registries) {
        ListTag list = new ListTag();
        for (GenericStack s : stacks) list.add(Stacks.save(s, registries));
        return list;
    }

    /** Stacks whose content no longer exists are dropped. */
    private static List<GenericStack> loadStacks(ListTag list, HolderLookup.Provider registries) {
        List<GenericStack> stacks = new ArrayList<>(list.size());
        for (Tag t : list) {
            GenericStack s = Stacks.load((CompoundTag) t, registries);
            if (s != null) stacks.add(s);
        }
        return stacks;
    }

    // ------------------------------------------------------------------ return inventory

    /**
     * What hoppers, transposers and pipes see: one empty slot that sends
     * whatever is put in it into the ME network, where waiting crafting jobs
     * take their outputs first. Items that don't fit are refused.
     */
    private final class ReturnInventory implements IItemHandler {
        @Override
        public int getSlots() {
            return 1;
        }

        @Override
        public ItemStack getStackInSlot(int slot) {
            return ItemStack.EMPTY;
        }

        @Override
        public ItemStack insertItem(int slot, ItemStack stack, boolean simulate) {
            if (stack.isEmpty()) return stack;
            AEItemKey key = AEItemKey.of(stack);
            if (key == null) return stack;
            long inserted = insertIntoNetwork(key, stack.getCount(), simulate ? Actionable.SIMULATE : Actionable.MODULATE);
            return inserted >= stack.getCount() ? ItemStack.EMPTY : stack.copyWithCount(stack.getCount() - (int) inserted);
        }

        @Override
        public ItemStack extractItem(int slot, int amount, boolean simulate) {
            return ItemStack.EMPTY;
        }

        @Override
        public int getSlotLimit(int slot) {
            return 64;
        }

        @Override
        public boolean isItemValid(int slot, ItemStack stack) {
            return true;
        }
    }
}
