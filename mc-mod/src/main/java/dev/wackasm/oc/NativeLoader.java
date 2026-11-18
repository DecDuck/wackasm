package dev.wackasm.oc;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.Locale;
import java.util.zip.CRC32;

/**
 * Loads {@code wackasm_oc}. Checked in order:
 * <ol>
 *   <li>{@code -Dwackasm.native=/abs/path/to/library}</li>
 *   <li>the copy bundled in the mod jar at {@code /natives/<os>-<arch>/}, which is
 *       extracted to a directory named by its checksum (Windows locks loaded
 *       DLLs, so a new build never has to overwrite an old one)</li>
 *   <li>{@code java.library.path}</li>
 * </ol>
 */
final class NativeLoader {
    private static final String NAME = "wackasm_oc";

    private NativeLoader() {}

    static void load() {
        String explicit = System.getProperty("wackasm.native");
        if (explicit != null) {
            System.load(explicit);
            return;
        }

        String file = System.mapLibraryName(NAME);
        String resource = "/natives/" + platform() + "/" + file;
        try (InputStream in = NativeLoader.class.getResourceAsStream(resource)) {
            if (in == null) {
                System.loadLibrary(NAME);
                return;
            }
            byte[] bytes = readAll(in);
            CRC32 crc = new CRC32();
            crc.update(bytes);
            File dir = new File(System.getProperty("java.io.tmpdir"),
                    "wackasm-oc-natives/" + Long.toHexString(crc.getValue()));
            File target = new File(dir, file);
            if (!target.isFile() || target.length() != bytes.length) {
                if (!dir.isDirectory() && !dir.mkdirs()) throw new IOException("cannot create " + dir);
                File tmp = File.createTempFile(NAME, ".tmp", dir);
                Files.write(tmp.toPath(), bytes);
                try {
                    Files.move(tmp.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE);
                } catch (IOException raced) {
                    // Another server in this JVM's temp dir got there first; theirs is identical.
                    tmp.delete();
                }
            }
            System.load(target.getAbsolutePath());
        } catch (IOException e) {
            throw new IllegalStateException("failed to extract " + resource, e);
        }
    }

    /** Matches the directory names the Gradle build bundles the library into. */
    static String platform() {
        String os = System.getProperty("os.name").toLowerCase(Locale.ROOT);
        String arch = System.getProperty("os.arch").toLowerCase(Locale.ROOT);
        String osName = os.contains("win") ? "windows" : os.contains("mac") ? "macos" : "linux";
        String archName = arch.equals("amd64") || arch.equals("x86_64") ? "x86_64"
                : arch.equals("aarch64") || arch.equals("arm64") ? "aarch64" : arch;
        return osName + "-" + archName;
    }

    static byte[] readAll(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buf = new byte[64 * 1024];
        for (int n; (n = in.read(buf)) > 0; ) out.write(buf, 0, n);
        return out.toByteArray();
    }
}
