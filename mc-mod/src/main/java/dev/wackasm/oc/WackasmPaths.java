package dev.wackasm.oc;

import java.io.File;

import net.minecraft.server.MinecraftServer;
import net.minecraft.world.level.storage.LevelResource;

/**
 * Picks the native work dir. It defaults to {@code <world save>/wackasm} so
 * snapshots travel with the world they belong to, while
 * {@code -Dwackasm.workdir} pins it anywhere.
 */
final class WackasmPaths {
    private WackasmPaths() {}

    /** Resolved on every call: a single-player client can switch worlds. */
    static String workDir() {
        String override = System.getProperty("wackasm.workdir");
        File dir;
        if (override != null) {
            dir = new File(override);
        } else {
            MinecraftServer server = WackasmMod.server;
            dir = server != null
                    ? server.getWorldPath(LevelResource.ROOT).resolve("wackasm").normalize().toFile()
                    : new File("wackasm");
        }
        return dir.getAbsoluteFile().getPath();
    }
}
