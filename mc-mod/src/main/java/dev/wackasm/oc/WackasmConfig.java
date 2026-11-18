package dev.wackasm.oc;

import java.io.IOException;
import java.io.Reader;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

import net.fabricmc.loader.api.FabricLoader;

/**
 * {@code config/wackasm.properties}, written with its defaults and comments on
 * first start. It's read when the game starts and again when each world (or
 * server) starts, so edits apply without restarting Minecraft. Values out of
 * range are clamped; unreadable ones fall back to the default.
 */
public final class WackasmConfig {
    public static volatile double callBudgetMultiplier = 1.0;
    public static volatile double memoryMultiplier = 1.8;
    public static volatile int asyncCallsPerTick = 1;
    public static volatile int fuelPerSlice = 5_000_000;

    private static final String DEFAULTS = String.join("\n",
            "# wackasm: WebAssembly computers for OpenComputers. Lua computers are never affected.",
            "",
            "# How much further the per-tick direct-call budget goes for WebAssembly computers.",
            "# OpenComputers charges each direct component call 1/limit of the budget (for example",
            "# 1/15 for filesystem.read). For wasm computers the charge is divided by this, so 2.0",
            "# allows twice as many direct calls per tick as a Lua computer with the same parts.",
            "# 1.0 matches Lua. Range 0.1 to 16.",
            "callBudgetMultiplier=1.0",
            "",
            "# How much memory WebAssembly computers get, as a multiple of their installed RAM.",
            "# 1.8 matches what OpenComputers gives Lua on a 64-bit server, so a wasm computer gets",
            "# the same memory as a Lua computer with the same parts. Each running wasm program",
            "# needs its own stack and data (about 128 KiB), where Lua programs share one VM.",
            "# Takes effect when a computer's parts change or it restarts. Range 1.0 to 4.0.",
            "memoryMultiplier=1.8",
            "",
            "# Indirect component calls a WebAssembly computer's async queue (the wackasm-opencomputers",
            "# `async` feature) may run per tick. Lua computers, and blocking calls, get one indirect",
            "# call per tick, so 1 keeps wasm at parity: the program still keeps running while its",
            "# calls wait. Raise it (16 is reasonable) to let programs overlap several indirect calls,",
            "# such as transposers or internet reads, in one tick. Range 1 to 64.",
            "asyncCallsPerTick=1",
            "",
            "# WebAssembly instructions per execution slice. A machine that uses up its slice",
            "# continues on the next tick. At least 10000.",
            "fuelPerSlice=5000000",
            "");

    private WackasmConfig() {}

    private static final String FILE_NAME = "wackasm.properties";

    static Path path() {
        return FabricLoader.getInstance().getConfigDir().resolve(FILE_NAME);
    }

    /** Reads the file, creating it with the defaults if it's missing. */
    static void load() {
        Path path = path();
        try {
            if (!Files.exists(path)) {
                Files.createDirectories(path.getParent());
                try (Writer out = Files.newBufferedWriter(path, StandardCharsets.UTF_8)) {
                    out.write(DEFAULTS);
                }
            }
            Properties props = new Properties();
            try (Reader in = Files.newBufferedReader(path, StandardCharsets.UTF_8)) {
                props.load(in);
            }
            apply(props);
        } catch (IOException e) {
            WackasmMod.log.warn("Couldn't read {}; using the defaults", path, e);
        }
    }

    /** Takes the settings from {@code props}, clamped to their ranges. */
    static void apply(Properties props) {
        callBudgetMultiplier = clamp(number(props, "callBudgetMultiplier", 1.0), 0.1, 16.0);
        memoryMultiplier = clamp(number(props, "memoryMultiplier", 1.8), 1.0, 4.0);
        asyncCallsPerTick = (int) clamp(number(props, "asyncCallsPerTick", 1), 1, 64);
        fuelPerSlice = (int) clamp(number(props, "fuelPerSlice", 5_000_000), 10_000, Integer.MAX_VALUE);
    }

    private static double number(Properties props, String key, double fallback) {
        String value = props.getProperty(key);
        if (value == null) return fallback;
        try {
            double d = Double.parseDouble(value.trim());
            return Double.isFinite(d) ? d : fallback;
        } catch (NumberFormatException e) {
            WackasmMod.log.warn("{} in {} isn't a number: {}", key, FILE_NAME, value);
            return fallback;
        }
    }

    private static double clamp(double v, double min, double max) {
        return Math.max(min, Math.min(max, v));
    }
}
