package dev.wackasm.oc;

import java.io.File;

import net.minecraftforge.common.DimensionManager;

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
            File save = DimensionManager.getCurrentSaveRootDirectory();
            dir = save != null ? new File(save, "wackasm") : new File("wackasm");
        }
        return dir.getAbsoluteFile().getPath();
    }
}
