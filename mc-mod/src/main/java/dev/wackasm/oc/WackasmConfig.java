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
        "WebAssembly instructions per execution slice. A machine that uses up its slice",
        "continues on the next tick."
    })
    @Config.RangeInt(min = 10_000)
    public static int fuelPerSlice = 5_000_000;

    private WackasmConfig() {}
}
