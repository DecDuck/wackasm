package dev.wackasm.oc;

import net.fabricmc.api.ClientModInitializer;

/**
 * Fabric runs client entrypoints after every mod's main entrypoint, so
 * OpenComputers' API is set up by now (see {@link WackasmMod}).
 */
public final class WackasmClient implements ClientModInitializer {
    @Override
    public void onInitializeClient() {
        WackasmMod.registerWithOpenComputers();
    }
}
