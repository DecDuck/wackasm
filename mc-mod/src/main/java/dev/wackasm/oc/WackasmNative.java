package dev.wackasm.oc;

/**
 * Raw bindings to the {@code wackasm_oc} native library (crates/wackasm-oc/src/jni.rs).
 *
 * <p>Rust never calls into Java. Each method is a short call that Java starts,
 * using primitives and byte arrays in the {@link OcValueCodec} encoding. Every
 * method throws {@link IllegalStateException} on failure, including native
 * panics. The constants mirror {@code src/abi.rs}.
 */
public final class WackasmNative {
    // run() results
    public static final int RUN_YIELD = 0;
    public static final int RUN_HALTED = 1;
    public static final int RUN_SHUTDOWN = 2;
    public static final int RUN_ERROR = 3;
    public static final int RUN_INVOKE = 4;
    public static final int RUN_PULL_SIGNAL = 5;
    public static final int RUN_PUSH_SIGNAL = 6;
    public static final int RUN_COMPONENTS = 7;
    public static final int RUN_METHODS = 8;
    public static final int RUN_BEEP = 9;
    public static final int RUN_DEFAULT_DISK = 10;

    // respond() statuses
    public static final int STATUS_OK = 0;
    public static final int STATUS_TIMEOUT = 1;
    public static final int STATUS_ERROR = -1;
    public static final int STATUS_BAD_ARGUMENT = -2;
    public static final int STATUS_NO_SUCH_COMPONENT = -3;
    public static final int STATUS_NO_SUCH_METHOD = -4;

    static {
        NativeLoader.load();
    }

    private WackasmNative() {}

    /**
     * Creates a machine. {@code key} names its files under {@code workDir} and
     * must be {@code [A-Za-z0-9_-]{1,64}}. {@code address} is the address the
     * guest sees until a snapshot is loaded.
     */
    public static native long create(String workDir, String key, String address);

    /** Frees a handle. Passing 0 is a no-op. */
    public static native void destroy(long handle);

    public static native void setMemoryLimit(long handle, long bytes);

    /** Starts the EEPROM's wasm code. Throws "no bios found; ..." if there is none, as Lua does. */
    public static native void boot(long handle, byte[] eepromOrNull);

    /** Runs up to {@code fuel} instructions. Returns a {@code RUN_*} code. */
    public static native int run(long handle, double uptime, long fuel);

    /** The encoded array describing the pending request or halt reason, or null. */
    public static native byte[] payload(long handle);

    /**
     * Completes the pending request. When {@code status} is {@link #STATUS_OK},
     * {@code payload} is an encoded value. Otherwise it is a UTF-8 message.
     */
    public static native void respond(long handle, int status, byte[] payloadOrNull);

    /** Writes the snapshot into the work dir; NBT only needs the key. */
    public static native void save(long handle);

    /** Restores the snapshot. Returns false if there is none. */
    public static native boolean load(long handle);

    /** Drops the program and deletes its snapshot. */
    public static native void discard(long handle);
}
