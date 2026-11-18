package dev.wackasm.oc;

import net.minecraftforge.common.config.Config;

/** {@code config/wackasm.cfg}. Forge loads it when the mod is constructed. */
@Config(modid = WackasmMod.MOD_ID)
public final class WackasmConfig {
    @Config.Comment({
        "How much further the per-tick direct-call budget goes for WebAssembly computers.",
        "OpenComputers charges each direct component call 1/limit of the budget (for example",
        "1/15 for filesystem.read). For wasm computers the charge is divided by this, so 2.0",
        "allows twice as many direct calls per tick as a Lua computer with the same parts.",
        "1.0 matches Lua. Lua computers are never affected."
    })
    @Config.RangeDouble(min = 0.1, max = 16.0)
    public static double callBudgetMultiplier = 1.0;

    @Config.Comment({
        "How much memory WebAssembly computers get, as a multiple of their installed RAM.",
        "1.8 matches what OpenComputers gives Lua on a 64-bit server (its ramScaleFor64Bit), so",
        "a wasm computer gets the same memory as a Lua computer with the same parts. Each running",
        "wasm program needs its own stack and data (about 128 KiB), where Lua programs share one",
        "VM. Lua computers are never affected. Takes effect when a computer's parts change or it",
        "restarts."
    })
    @Config.RangeDouble(min = 1.0, max = 4.0)
    public static double memoryMultiplier = 1.8;

    @Config.Comment({
        "WebAssembly instructions per execution slice. A machine that uses up its slice",
        "continues on the next tick."
    })
    @Config.RangeInt(min = 10_000)
    public static int fuelPerSlice = 5_000_000;

    private WackasmConfig() {}
}
