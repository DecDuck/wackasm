package dev.wackasm.oc;

import static org.junit.jupiter.api.Assertions.*;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.util.Map;

import li.cil.oc.api.fs.Handle;
import li.cil.oc.api.fs.Mode;
import net.minecraft.nbt.CompoundTag;
import org.junit.jupiter.api.Test;

/** {@link FloppyFileSystem} over an index and in-memory contents. */
class FloppyFileSystemTest {
    private static final Map<String, String> CONTENTS = Map.of(
            "init.wasm", "INIT",
            "bin/hello.wasm", "hello world",
            "bin/tools/x.wasm", "x");

    private static FloppyFileSystem fs() {
        String index = "init.wasm\t4\t100\nbin/hello.wasm\t11\t200\nbin/tools/x.wasm\t1\t300\n";
        return new FloppyFileSystem(FloppyFileSystem.parseIndex(index),
                path -> CONTENTS.containsKey(path)
                        ? new ByteArrayInputStream(CONTENTS.get(path).getBytes(StandardCharsets.UTF_8)) : null);
    }

    @Test
    void describesTheTree() {
        FloppyFileSystem fs = fs();
        assertTrue(fs.exists("init.wasm") && fs.exists("/bin/") && fs.exists(""));
        assertFalse(fs.exists("nope"));
        assertTrue(fs.isDirectory("bin/tools") && !fs.isDirectory("init.wasm"));
        assertEquals(11, fs.size("/bin/hello.wasm"));
        assertEquals(200, fs.lastModified("bin/hello.wasm"));
        assertArrayEquals(new String[] {"bin/", "init.wasm"}, fs.list(""));
        assertArrayEquals(new String[] {"hello.wasm", "tools/"}, fs.list("bin"));
        assertNull(fs.list("init.wasm"));
        assertTrue(fs.isReadOnly());
        assertEquals(16, fs.spaceTotal());
    }

    @Test
    void readsAndSeeks() {
        FloppyFileSystem fs = fs();
        Handle h = fs.getHandle(fs.open("bin/hello.wasm", Mode.Read));
        byte[] buf = new byte[5];
        assertEquals(5, h.read(buf));
        assertEquals("hello", new String(buf, StandardCharsets.UTF_8));
        assertEquals(6, h.seek(6));
        assertEquals(5, h.read(buf));
        assertEquals(-1, h.read(buf));
        h.close();
        assertThrows(RuntimeException.class, () -> fs.open("nope", Mode.Read));
        assertThrows(RuntimeException.class, () -> fs.open("init.wasm", Mode.Write));
        assertFalse(fs.delete("init.wasm") || fs.makeDirectory("new"));
    }

    @Test
    void theRealFloppyLoadsFromTheClassPath() {
        // As in a dev run, the test's classes and resources are separate directories.
        FloppyFileSystem fs = FloppyFileSystem.fromResources();
        assertTrue(fs.exists("/init.wasm"), "init.wasm is on the floppy");
        Handle h = fs.getHandle(fs.open("/init.wasm", Mode.Read));
        byte[] magic = new byte[4];
        assertEquals(4, h.read(magic));
        assertArrayEquals(new byte[] {0, 'a', 's', 'm'}, magic);
        assertEquals(fs.size("init.wasm"), h.length());
        assertTrue(java.util.Arrays.asList(fs.list("")).contains("bin/"));
    }

    @Test
    void openHandlesSurviveASave() {
        FloppyFileSystem fs = fs();
        int id = fs.open("bin/hello.wasm", Mode.Read);
        fs.getHandle(id).seek(6);
        CompoundTag nbt = new CompoundTag();
        fs.save(nbt, null);

        FloppyFileSystem restored = fs();
        restored.load(nbt, null);
        byte[] buf = new byte[5];
        assertEquals(5, restored.getHandle(id).read(buf));
        assertEquals("world", new String(buf, StandardCharsets.UTF_8));
        // New handles don't reuse the restored id.
        assertNotEquals(id, restored.open("init.wasm", Mode.Read));
    }
}
