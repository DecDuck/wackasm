package dev.wackasm.oc;

import java.io.File;
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
import li.cil.oc.api.network.Component;
import li.cil.oc.api.network.Node;
import net.minecraft.item.ItemStack;
import net.minecraft.nbt.NBTTagCompound;

/**
 * An OpenComputers architecture that runs WebAssembly on wackasm.
 *
 * <p>Register it during mod init with {@code li.cil.oc.api.Machine.add(WackasmArchitecture.class)}.
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
 * <p>System properties:
 * <ul>
 *   <li>{@code wackasm.workdir}: native work dir (default {@code ./wackasm}). Holds snapshots, logs and an optional {@code boot.wasm}.</li>
 *   <li>{@code wackasm.fuel}: instructions per {@code runThreaded} slice (default 5,000,000).</li>
 * </ul>
 */
@Architecture.Name("WebAssembly (wackasm)")
public final class WackasmArchitecture implements Architecture {
    private static final String NBT_KEY = "wackasm:key";
    private static final String WORK_DIR =
            new File(System.getProperty("wackasm.workdir", "wackasm")).getAbsolutePath();
    private static final long FUEL_PER_SLICE = Long.getLong("wackasm.fuel", 5_000_000L);

    private final Machine machine;
    /** Names this machine's snapshot. It is stable across reloads, unlike the handle. */
    private String key = UUID.randomUUID().toString();
    private long handle;
    private boolean initialized;
    private long memoryBytes;

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
        memoryBytes = bytes;
        if (handle != 0) WackasmNative.setMemoryLimit(handle, bytes);
        return bytes > 0;
    }

    @Override
    public boolean initialize() {
        close();
        try {
            // Reuse the key across reboots. boot() deletes the stale snapshot.
            handle = WackasmNative.create(WORK_DIR, key, machine.node().address());
            WackasmNative.setMemoryLimit(handle, memoryBytes);
            WackasmNative.boot(handle, readEeprom());
            initialized = true;
            return true;
        } catch (Throwable t) {
            machine.crash("wackasm: " + t.getMessage());
            close();
            return false;
        }
    }

    @Override
    public void close() {
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
            handle = WackasmNative.create(WORK_DIR, key, machine.node().address());
            WackasmNative.setMemoryLimit(handle, memoryBytes);
            initialized = WackasmNative.load(handle);
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
        if (handle == 0) return new ExecutionResult.Error("wackasm: machine state was lost");
        try {
            while (true) {
                int code = WackasmNative.run(handle, machine.upTime(), FUEL_PER_SLICE);
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
            invoke((String) p[0], (String) p[1], (Object[]) p[2]);
        } catch (LimitReachedException e) {
            // Synchronized calls aren't budgeted, so this shouldn't happen. If it
            // does, the request stays pending and runThreaded reissues it.
        } catch (IllegalStateException e) {
            machine.crash("wackasm: " + e.getMessage());
        }
    }

    // ---------------------------------------------------------------- helpers

    private Object[] payload() {
        byte[] bytes = WackasmNative.payload(handle);
        if (bytes == null) throw new IllegalStateException("no pending request");
        return OcValueCodec.decodeArray(bytes);
    }

    private void respondOk(Object value) {
        WackasmNative.respond(handle, WackasmNative.STATUS_OK, value == null ? null : OcValueCodec.encode(value));
    }

    private void respondStatus(int status, String message) {
        WackasmNative.respond(handle, status, message == null ? null : message.getBytes(StandardCharsets.UTF_8));
    }

    /** Performs a component call and responds with its results or its error. */
    private void invoke(String address, String method, Object[] args) throws LimitReachedException {
        try {
            respondOk(machine.invoke(address, method, args));
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

    /** Fetches the EEPROM contents (OC's {@code eeprom.get}, a direct callback). */
    private byte[] readEeprom() {
        for (Map.Entry<String, String> e : machine.components().entrySet()) {
            if (!"eeprom".equals(e.getValue())) continue;
            try {
                Object[] result = machine.invoke(e.getKey(), "get", new Object[0]);
                if (result != null && result.length > 0 && result[0] instanceof byte[]) return (byte[]) result[0];
            } catch (Exception ignored) {
                // Fall through to the work-dir boot image.
            }
        }
        return null;
    }

    private static short clampShort(long v) {
        return (short) Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, v));
    }
}
