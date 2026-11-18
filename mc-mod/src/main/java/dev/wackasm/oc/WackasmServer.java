package dev.wackasm.oc;

import net.fabricmc.api.DedicatedServerModInitializer;

/**
 * Fabric runs dedicated server entrypoints after every mod's main entrypoint,
 * so OpenComputers' API is set up by now (see {@link WackasmMod}).
 */
public final class WackasmServer implements DedicatedServerModInitializer {
    @Override
    public void onInitializeServer() {
        WackasmMod.registerWithOpenComputers();
    }
}
