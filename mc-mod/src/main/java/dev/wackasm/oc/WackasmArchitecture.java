package dev.wackasm.oc;

import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.UUID;

import li.cil.oc.api.Driver;
import li.cil.oc.api.driver.DriverItem;
import li.cil.oc.api.driver.item.Memory;
import li.cil.oc.api.machine.Architecture;
import li.cil.oc.api.machine.Callback;
import li.cil.oc.api.machine.ExecutionResult;
import li.cil.oc.api.machine.LimitReachedException;
import li.cil.oc.api.machine.Machine;
import li.cil.oc.api.machine.Signal;
import li.cil.oc.api.machine.Value;
import li.cil.oc.api.network.Component;
import li.cil.oc.api.network.Node;
import net.minecraft.item.ItemStack;
import net.minecraft.nbt.NBTTagCompound;

/**
 * An OpenComputers architecture that runs WebAssembly on wackasm.
 *
 * <p>{@link WackasmMod} registers it with OpenComputers. Any CPU can switch to
 * it: sneak and right-click the CPU item to cycle architectures.
 *
 * <p>Execution model: {@link #runThreaded} calls {@code run()} in a loop. When
 * the guest needs the game, the native side suspends the guest with its import
 * pending and returns a request code. The request is either served inline
 * (direct callbacks, signals, listings) or deferred to {@link #runSynchronized}
 * by returning {@link ExecutionResult.SynchronizedCall} (non-direct callbacks).
 * Either way it ends with {@code respond()}, which resumes the guest. The
 * suspended guest, including any pending request, is serialized into the
 * native work dir on save, and NBT only stores the key that names it.
 *
 * <p>Settings live in {@link WackasmConfig} ({@code config/wackasm.cfg}). System properties:
 * <ul>
 *   <li>{@code wackasm.workdir}: native work dir (default {@code <world>/wackasm}); see {@link WackasmPaths}.</li>
 * </ul>
 */
@Architecture.Name("WebAssembly (wackasm)")
public final class WackasmArchitecture implements Architecture {
    private static final String NBT_KEY = "wackasm:key";

    private final Machine machine;
    /** Names this machine's snapshot. It is stable across reloads, unlike the handle. */
    private String key = UUID.randomUUID().toString();
    private long handle;
    private boolean initialized;
    private long memoryBytes;
    /** Host objects (file handles and so on) the guest holds by id. */
    private final UserdataTable userdata = new UserdataTable();
    /** Why the last boot failed. Reported from runThreaded, where OC shows it to the player. */
    private String bootError;

    public WackasmArchitecture(Machine machine) {
        this.machine = machine;
    }

    // ---------------------------------------------------------------- lifecycle

    @Override
    public boolean isInitialized() {
        return initialized;
    }

    @Override
    public boolean recomputeMemory(Iterable<ItemStack> components) {
        long bytes = 0;
        for (ItemStack stack : components) {
            DriverItem driver = Driver.driverFor(stack, machine.host().getClass());
            if (driver instanceof Memory) {
                bytes += (long) (((Memory) driver).amount(stack) * 1024);
            }
        }
        memoryBytes = memoryLimit(bytes, WackasmConfig.memoryMultiplier);
        if (handle != 0) WackasmNative.setMemoryLimit(handle, memoryBytes);
        return bytes > 0;
    }

    /**
     * The guest's memory budget for {@code installed} bytes of RAM, scaled by
     * {@link WackasmConfig#memoryMultiplier} (clamped to its 1.0 to 4.0 range).
     * Left in bytes: the guest can grow by whole 64 KiB pages that fit.
     */
    static long memoryLimit(long installed, double multiplier) {
        double scale = Double.isNaN(multiplier) ? 1.0 : Math.max(1.0, Math.min(4.0, multiplier));
        return (long) (installed * scale);
    }

    @Override
    public boolean initialize() {
        close();
        try {
            // Reuse the key across reboots. boot() deletes the stale snapshot.
            handle = WackasmNative.create(WackasmPaths.workDir(), key, machine.node().address());
            WackasmNative.setMemoryLimit(handle, memoryBytes);
            WackasmNative.boot(handle, readEeprom());
        } catch (Throwable t) {
            // Like a missing Lua BIOS: start, then halt with the reason (for
            // example "no bios found; install a configured EEPROM"), so the
            // player sees it in the computer's GUI.
            bootError = t.getMessage();
        }
        initialized = true;
        return true;
    }

    @Override
    public void close() {
        userdata.disposeAll(machine);
        bootError = null;
        if (handle != 0) {
            WackasmNative.destroy(handle);
            handle = 0;
        }
        initialized = false;
    }

    @Override
    public void save(NBTTagCompound nbt) {
        if (handle == 0) return;
        try {
            WackasmNative.save(handle);
            nbt.setString(NBT_KEY, key);
            // Saved together with the snapshot, so the ids in it stay valid.
            NBTTagCompound values = new NBTTagCompound();
            userdata.save(values);
            nbt.setTag("wackasm:userdata", values);
        } catch (IllegalStateException e) {
            // Without a snapshot the machine restarts after the next load.
            machine.crash("wackasm: failed to save: " + e.getMessage());
        }
    }

    @Override
    public void load(NBTTagCompound nbt) {
        close();
        if (!nbt.hasKey(NBT_KEY)) return;
        key = nbt.getString(NBT_KEY);
        try {
            handle = WackasmNative.create(WackasmPaths.workDir(), key, machine.node().address());
            WackasmNative.setMemoryLimit(handle, memoryBytes);
            initialized = WackasmNative.load(handle);
            if (initialized) userdata.load(nbt.getCompoundTag("wackasm:userdata"));
        } catch (IllegalStateException e) {
            initialized = false;
        }
        if (!initialized) close();
    }

    @Override
    public void onSignal() {
        // Nothing to do: OC wakes a sleeping machine on a signal, and the next
        // runThreaded answers the pending pull_signal.
    }

    @Override
    public void onConnect() {}

    // ---------------------------------------------------------------- execution

    @Override
    public ExecutionResult runThreaded(boolean isSynchronizedReturn) {
        if (bootError != null) return new ExecutionResult.Error(bootError);
        if (handle == 0) return new ExecutionResult.Error("wackasm: machine state was lost");
        try {
            while (true) {
                int code = WackasmNative.run(handle, machine.upTime(), WackasmConfig.fuelPerSlice);
                switch (code) {
                    case WackasmNative.RUN_YIELD:
                        return new ExecutionResult.Sleep(0);
                    case WackasmNative.RUN_HALTED:
                        return new ExecutionResult.Shutdown(false);
                    case WackasmNative.RUN_SHUTDOWN:
                        return new ExecutionResult.Shutdown(Boolean.TRUE.equals(payload()[0]));
                    case WackasmNative.RUN_ERROR:
                        return new ExecutionResult.Error(String.valueOf(payload()[0]));
                    case WackasmNative.RUN_INVOKE: {
                        Object[] p = payload();
                        String address = (String) p[0], method = (String) p[1];
                        if (!isDirect(address, method)) return new ExecutionResult.SynchronizedCall();
                        try {
                            invoke(address, method, (Object[]) p[2]);
                        } catch (LimitReachedException e) {
                            // Direct-call budget for this tick is spent. Leave the
                            // request pending and retry next tick.
                            return new ExecutionResult.Sleep(0);
                        }
                        break;
                    }
                    case WackasmNative.RUN_INVOKE_VALUE: {
                        Object[] p = payload();
                        Value value = p[0] instanceof Value ? (Value) p[0] : null;
                        if (value == null) {
                            respondStatus(WackasmNative.STATUS_NO_SUCH_COMPONENT, "no such value (already released?)");
                            break;
                        }
                        if (!isDirect(value, (String) p[1])) return new ExecutionResult.SynchronizedCall();
                        try {
                            invokeValue(value, (String) p[1], (Object[]) p[2]);
                        } catch (LimitReachedException e) {
                            return new ExecutionResult.Sleep(0);
                        }
                        break;
                    }
                    case WackasmNative.RUN_INVOKE_ASYNC: {
                        Object[] p = payload();
                        if (!isDirectAsync(p)) return new ExecutionResult.SynchronizedCall();
                        try {
                            invokeAsync(p);
                        } catch (LimitReachedException e) {
                            return new ExecutionResult.Sleep(0); // reported again next tick
                        }
                        break;
                    }
                    case WackasmNative.RUN_RELEASE_VALUE: {
                        Object value = payload()[0];
                        if (value instanceof Value) userdata.release((Value) value, machine);
                        respondOk(null);
                        break;
                    }
                    case WackasmNative.RUN_PULL_SIGNAL: {
                        Signal signal = machine.popSignal();
                        if (signal != null) {
                            Object[] args = signal.args();
                            Object[] encoded = new Object[args.length + 1];
                            encoded[0] = signal.name();
                            System.arraycopy(args, 0, encoded, 1, args.length);
                            respondOk(encoded);
                            break;
                        }
                        Object remaining = payload()[0];
                        if (remaining instanceof Double && (Double) remaining <= 0) {
                            WackasmNative.respond(handle, WackasmNative.STATUS_TIMEOUT, null);
                            break;
                        }
                        int ticks = remaining == null
                                ? Integer.MAX_VALUE
                                : (int) Math.max(1, Math.ceil((Double) remaining * 20));
                        return new ExecutionResult.Sleep(ticks);
                    }
                    case WackasmNative.RUN_PUSH_SIGNAL: {
                        Object[] p = payload();
                        Object[] args = new Object[p.length - 1];
                        System.arraycopy(p, 1, args, 0, args.length);
                        boolean queued = machine.signal((String) p[0], args);
                        respondStatus(queued ? WackasmNative.STATUS_OK : WackasmNative.STATUS_ERROR,
                                queued ? null : "signal queue is full");
                        break;
                    }
                    case WackasmNative.RUN_COMPONENTS: {
                        String filter = (String) payload()[0];
                        Map<String, String> out = new LinkedHashMap<>();
                        for (Map.Entry<String, String> e : machine.components().entrySet()) {
                            if (filter.isEmpty() || e.getValue().contains(filter)) out.put(e.getKey(), e.getValue());
                        }
                        respondOk(out);
                        break;
                    }
                    case WackasmNative.RUN_METHODS: {
                        Component component = component((String) payload()[0]);
                        if (component == null) {
                            respondStatus(WackasmNative.STATUS_NO_SUCH_COMPONENT, "no such component");
                            break;
                        }
                        Map<String, Object> out = new LinkedHashMap<>();
                        for (String name : component.methods()) {
                            Callback cb = component.annotation(name);
                            Map<String, Object> info = new LinkedHashMap<>();
                            info.put("direct", cb != null && cb.direct());
                            info.put("doc", cb != null ? cb.doc() : "");
                            out.put(name, info);
                        }
                        respondOk(out);
                        break;
                    }
                    case WackasmNative.RUN_BEEP: {
                        Object[] p = payload();
                        machine.beep(clampShort((Long) p[0]), clampShort((Long) p[1]));
                        respondOk(null);
                        break;
                    }
                    case WackasmNative.RUN_DEFAULT_DISK: {
                        String disk;
                        try {
                            disk = defaultDisk();
                        } catch (LimitReachedException e) {
                            return new ExecutionResult.Sleep(0); // retried next tick
                        }
                        respondOk(disk == null ? new Object[0] : new Object[] {disk});
                        break;
                    }
                    default:
                        return new ExecutionResult.Error("wackasm: unknown run code " + code);
                }
            }
        } catch (IllegalStateException e) {
            return new ExecutionResult.Error("wackasm: " + e.getMessage());
        }
    }

    /** Serves a non-direct component call on the server thread. */
    @Override
    public void runSynchronized() {
        if (handle == 0) return;
        try {
            Object[] p = payload();
            if (p[0] instanceof Long) {
                serveAsyncQueue(p);
            } else if (p[0] instanceof Value) {
                invokeValue((Value) p[0], (String) p[1], (Object[]) p[2]);
            } else {
                invoke((String) p[0], (String) p[1], (Object[]) p[2]);
            }
        } catch (LimitReachedException e) {
            // Synchronized calls aren't budgeted, so this shouldn't happen. If it
            // does, the request stays pending and runThreaded reissues it.
        } catch (IllegalStateException e) {
            machine.crash("wackasm: " + e.getMessage());
        }
    }

    /**
     * Serves the async queue from the server thread, starting with the pending
     * call {@code p}: up to {@link WackasmConfig#asyncCallsPerTick} indirect calls,
     * and any direct calls between them. A call left over stays pending, and
     * runThreaded reports it again next tick.
     */
    private void serveAsyncQueue(Object[] p) throws LimitReachedException {
        int indirect = 0;
        while (true) {
            boolean direct = isDirectAsync(p);
            if (!direct && indirect >= WackasmConfig.asyncCallsPerTick) return;
            invokeAsync(p);
            if (!direct) indirect++;
            if (!WackasmNative.nextAsync(handle)) return;
            p = payload();
        }
    }

    /** {@code [ticket, target, method, args]}: target is an address or a userdata value. */
    private void invokeAsync(Object[] p) throws LimitReachedException {
        Object target = p[1];
        String method = (String) p[2];
        Object[] args = (Object[]) p[3];
        if (target instanceof Value) {
            invokeValue((Value) target, method, args);
        } else if (target instanceof String) {
            invoke((String) target, method, args);
        } else {
            respondStatus(WackasmNative.STATUS_NO_SUCH_COMPONENT, "no such value (already released?)");
        }
    }

    private boolean isDirectAsync(Object[] p) {
        Object target = p[1];
        String method = (String) p[2];
        if (target instanceof Value) return isDirect((Value) target, method);
        if (target instanceof String) return isDirect((String) target, method);
        return true; // an unknown value fails inline
    }

    // ---------------------------------------------------------------- helpers

    private Object[] payload() {
        byte[] bytes = WackasmNative.payload(handle);
        if (bytes == null) throw new IllegalStateException("no pending request");
        return OcValueCodec.decodeArray(bytes, userdata);
    }

    private void respondOk(Object value) {
        WackasmNative.respond(handle, WackasmNative.STATUS_OK, value == null ? null : OcValueCodec.encode(value, userdata));
    }

    private void respondStatus(int status, String message) {
        WackasmNative.respond(handle, status, message == null ? null : message.getBytes(StandardCharsets.UTF_8));
    }

    /** Performs a component call and responds with its results or its error. */
    private void invoke(String address, String method, Object[] args) throws LimitReachedException {
        try {
            respondOk(call(address, method, args));
        } catch (LimitReachedException e) {
            throw e;
        } catch (NoSuchMethodException e) {
            respondStatus(WackasmNative.STATUS_NO_SUCH_METHOD, "no such method: " + method);
        } catch (Throwable t) {
            boolean missing = component(address) == null;
            respondStatus(missing ? WackasmNative.STATUS_NO_SUCH_COMPONENT : WackasmNative.STATUS_ERROR,
                    missing ? "no such component" : String.valueOf(t.getMessage()));
        }
    }

    /**
     * Calls a method of a userdata value, such as an internet card's HTTP
     * request. This goes through {@link Machine#invoke(Value, String, Object[])}
     * as for Lua, so direct callbacks cost the usual budget
     * ({@link WackasmConfig#callBudgetMultiplier} does not apply; the internet
     * card's direct value callbacks are only {@code response}, {@code close}
     * and {@code id}).
     */
    private void invokeValue(Value value, String method, Object[] args) throws LimitReachedException {
        try {
            respondOk(machine.invoke(value, method, args));
        } catch (LimitReachedException e) {
            throw e;
        } catch (NoSuchMethodException e) {
            respondStatus(WackasmNative.STATUS_NO_SUCH_METHOD, "no such method: " + method);
        } catch (Throwable t) {
            respondStatus(WackasmNative.STATUS_ERROR, String.valueOf(t.getMessage()));
        }
    }

    private boolean isDirect(Value value, String method) {
        Callback cb = machine.methods(value).get(method);
        return cb == null || cb.direct(); // unknown methods fail inline
    }

    /**
     * The filesystem WASI programs see as {@code /}, the first usable disk:
     * <ol>
     *   <li>the boot disk (the BIOS records it in the EEPROM's data), if writable;</li>
     *   <li>else the first writable filesystem. When booting from the read-only
     *       shell floppy, that is usually the hard drive;</li>
     *   <li>else the boot disk, read-only;</li>
     *   <li>else the first filesystem at all.</li>
     * </ol>
     * The computer's tmpfs is never chosen. Filesystems are taken in address
     * order, so the choice is stable while the same disks are installed.
     */
    private String defaultDisk() throws LimitReachedException {
        Map<String, String> components = machine.components();
        String tmp = machine.tmpAddress();
        java.util.List<String> disks = new java.util.ArrayList<>();
        for (Map.Entry<String, String> e : components.entrySet()) {
            if ("filesystem".equals(e.getValue()) && !e.getKey().equals(tmp)) disks.add(e.getKey());
        }
        java.util.Collections.sort(disks);

        String boot = bootAddress(components);
        if (boot != null && disks.remove(boot)) disks.add(0, boot);
        for (String disk : disks) {
            if (!readOnly(disk)) return disk;
        }
        return disks.isEmpty() ? null : disks.get(0);
    }

    private String bootAddress(Map<String, String> components) throws LimitReachedException {
        for (Map.Entry<String, String> e : components.entrySet()) {
            if (!"eeprom".equals(e.getValue())) continue;
            try {
                Object[] data = call(e.getKey(), "getData", new Object[0]);
                if (data != null && data.length > 0 && data[0] instanceof byte[]) {
                    return new String((byte[]) data[0], StandardCharsets.UTF_8);
                }
            } catch (LimitReachedException limit) {
                throw limit;
            } catch (Exception ignored) {
                // No usable boot address.
            }
        }
        return null;
    }

    private boolean readOnly(String disk) throws LimitReachedException {
        try {
            Object[] result = call(disk, "isReadOnly", new Object[0]);
            return result != null && result.length > 0 && Boolean.TRUE.equals(result[0]);
        } catch (LimitReachedException limit) {
            throw limit;
        } catch (Exception e) {
            return true;
        }
    }

    /**
     * {@link Machine#invoke}, but direct calls draw on the call budget at
     * {@link WackasmConfig#callBudgetMultiplier} times the usual rate. This
     * does what OpenComputers' {@code Machine.invoke} does for a component
     * address: refuse while disconnected, require the component to be visible
     * to this machine, charge direct callbacks {@code 1/limit} of the budget,
     * and call {@link Component#invoke}. The only difference is the charge.
     */
    Object[] call(String address, String method, Object[] args) throws Exception {
        double multiplier = WackasmConfig.callBudgetMultiplier;
        if (multiplier == 1.0) return machine.invoke(address, method, args);

        Node self = machine.node();
        if (self == null || self.network() == null) throw new LimitReachedException();
        Node node = self.network().node(address);
        if (!(node instanceof Component)) throw new IllegalArgumentException("no such component");
        Component component = (Component) node;
        if (!component.canBeSeenFrom(self) && component != self) {
            throw new IllegalArgumentException("no such component");
        }
        Callback callback = component.annotation(method);
        if (callback != null && callback.direct()) {
            // Throws LimitReachedException once the tick's budget is spent.
            machine.consumeCallBudget(1.0 / callback.limit() / multiplier);
        }
        return component.invoke(method, machine, args);
    }

    private Component component(String address) {
        if (address == null || !machine.components().containsKey(address)) return null;
        Node node = machine.node().network() == null ? null : machine.node().network().node(address);
        return node instanceof Component ? (Component) node : null;
    }

    private boolean isDirect(String address, String method) {
        Component component = component(address);
        if (component == null) return true; // let invoke() report the missing component inline
        Callback cb = component.annotation(method);
        return cb == null || cb.direct(); // unknown methods also fail inline
    }

    /** The first EEPROM's code (OC's {@code eeprom.get}, a direct callback), or null if there is none. */
    private byte[] readEeprom() {
        for (Map.Entry<String, String> e : machine.components().entrySet()) {
            if (!"eeprom".equals(e.getValue())) continue;
            try {
                Object[] result = call(e.getKey(), "get", new Object[0]);
                if (result != null && result.length > 0 && result[0] instanceof byte[]) return (byte[]) result[0];
            } catch (Exception ignored) {
                // Unreadable EEPROM: boot fails with "no bios found".
            }
        }
        return null;
    }

    private static short clampShort(long v) {
        return (short) Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, v));
    }
}
