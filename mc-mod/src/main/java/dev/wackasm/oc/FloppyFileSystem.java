package dev.wackasm.oc;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import java.util.function.Function;

import li.cil.oc.api.fs.FileSystem;
import li.cil.oc.api.fs.Handle;
import li.cil.oc.api.fs.Mode;
import net.minecraft.core.HolderLookup;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.Tag;

/**
 * The shell floppy's read-only file system, served from the mod's own
 * resources ({@code assets/wackasm/loot/shell/}).
 *
 * <p>OpenComputers' {@code FileSystem.fromClass} looks for the files next to
 * the mod's classes. In a dev run, classes and resources are in
 * separate directories, so it finds an empty disk. This reads each file
 * through the class loader instead, which works the same from the jar and in
 * dev, using the table of contents the build writes ({@code shell.index}: one
 * {@code path, size, modified} line per file).
 *
 * <p>Paths are relative to the disk's root, as OpenComputers passes them.
 * Directories are listed with a trailing {@code /}. Open handles are saved
 * with the world, as for OpenComputers' own read-only file systems.
 */
final class FloppyFileSystem implements FileSystem {
    static final String ROOT = "/assets/wackasm/loot/shell/";
    static final String INDEX = "/assets/wackasm/loot/shell.index";

    private record Entry(long size, long modified) {}

    /** Files by path. */
    private final Map<String, Entry> files;
    /** Every directory, "" being the root. */
    private final TreeSet<String> directories = new TreeSet<>();
    private final long totalSize;
    /** Opens a file's contents by path. */
    private final Function<String, InputStream> opener;
    private final Map<Integer, OpenFile> handles = new HashMap<>();
    private int nextHandle = 1;

    FloppyFileSystem(Map<String, long[]> index, Function<String, InputStream> opener) {
        this.opener = opener;
        this.files = new LinkedHashMap<>();
        long total = 0;
        directories.add("");
        for (Map.Entry<String, long[]> e : index.entrySet()) {
            String path = clean(e.getKey());
            files.put(path, new Entry(e.getValue()[0], e.getValue()[1]));
            total += e.getValue()[0];
            for (int slash = path.indexOf('/'); slash >= 0; slash = path.indexOf('/', slash + 1)) {
                directories.add(path.substring(0, slash));
            }
        }
        this.totalSize = total;
    }

    /** The floppy as built into the mod jar. */
    static FloppyFileSystem fromResources() {
        FloppyFileSystem fs = new FloppyFileSystem(readIndex(), path -> WackasmMod.class.getResourceAsStream(ROOT + path));
        WackasmMod.log.debug("Shell floppy file system: {} files, {} bytes", fs.files.size(), fs.totalSize);
        return fs;
    }

    static Map<String, long[]> readIndex() {
        try (InputStream in = WackasmMod.class.getResourceAsStream(INDEX)) {
            if (in == null) throw new IllegalStateException("missing " + INDEX + " in the mod jar");
            return parseIndex(new String(NativeLoader.readAll(in), StandardCharsets.UTF_8));
        } catch (IOException e) {
            throw new IllegalStateException("reading " + INDEX, e);
        }
    }

    static Map<String, long[]> parseIndex(String text) {
        Map<String, long[]> index = new LinkedHashMap<>();
        for (String line : text.split("\n")) {
            String[] parts = line.trim().split("\t");
            if (parts.length != 3) continue;
            index.put(parts[0], new long[] {Long.parseLong(parts[1]), Long.parseLong(parts[2])});
        }
        return index;
    }

    /** "/bin/x.wasm/" to "bin/x.wasm"; "/" to "". */
    private static String clean(String path) {
        String p = path.replace('\\', '/');
        while (p.startsWith("/")) p = p.substring(1);
        while (p.endsWith("/")) p = p.substring(0, p.length() - 1);
        return p;
    }

    @Override public boolean isReadOnly() { return true; }
    @Override public long spaceTotal() { return totalSize; }
    @Override public long spaceUsed() { return totalSize; }

    @Override
    public boolean exists(String path) {
        String p = clean(path);
        return files.containsKey(p) || directories.contains(p);
    }

    @Override
    public long size(String path) {
        Entry e = files.get(clean(path));
        return e == null ? 0 : e.size();
    }

    @Override
    public boolean isDirectory(String path) {
        return directories.contains(clean(path));
    }

    @Override
    public long lastModified(String path) {
        Entry e = files.get(clean(path));
        return e == null ? 0 : e.modified();
    }

    @Override
    public String[] list(String path) {
        String dir = clean(path);
        if (!directories.contains(dir)) return null;
        String prefix = dir.isEmpty() ? "" : dir + "/";
        TreeSet<String> names = new TreeSet<>();
        for (String f : files.keySet()) {
            if (!f.startsWith(prefix)) continue;
            String rest = f.substring(prefix.length());
            int slash = rest.indexOf('/');
            names.add(slash < 0 ? rest : rest.substring(0, slash + 1));
        }
        return names.toArray(new String[0]);
    }

    @Override public boolean delete(String path) { return false; }
    @Override public boolean makeDirectory(String path) { return false; }
    @Override public boolean rename(String from, String to) { return false; }
    @Override public boolean setLastModified(String path, long time) { return false; }

    @Override
    public synchronized int open(String path, Mode mode) {
        String p = clean(path);
        if (mode != Mode.Read || !files.containsKey(p)) {
            throw new RuntimeException(new FileNotFoundException(path));
        }
        byte[] data = read(p);
        int id = nextHandle++;
        handles.put(id, new OpenFile(id, p, data));
        return id;
    }

    private byte[] read(String path) {
        try (InputStream in = opener.apply(path)) {
            if (in == null) {
                WackasmMod.log.warn("Shell floppy: {} is in the index but not in the mod's resources", path);
                throw new RuntimeException(new FileNotFoundException(path));
            }
            return NativeLoader.readAll(in);
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
    }

    @Override
    public synchronized Handle getHandle(int handle) {
        return handles.get(handle);
    }

    @Override
    public synchronized void close() {
        handles.clear();
    }

    @Override
    public synchronized void load(CompoundTag nbt, HolderLookup.Provider provider) {
        handles.clear();
        ListTag list = nbt.getList("input", Tag.TAG_COMPOUND);
        for (int i = 0; i < list.size(); i++) {
            CompoundTag h = list.getCompound(i);
            String path = h.getString("path");
            if (!files.containsKey(path)) continue; // a newer floppy no longer has it
            OpenFile file = new OpenFile(h.getInt("handle"), path, read(path));
            file.seek(h.getLong("position"));
            handles.put(file.id, file);
            nextHandle = Math.max(nextHandle, file.id + 1);
        }
    }

    @Override
    public synchronized void save(CompoundTag nbt, HolderLookup.Provider provider) {
        ListTag list = new ListTag();
        List<OpenFile> open = new ArrayList<>(handles.values());
        for (OpenFile file : open) {
            CompoundTag h = new CompoundTag();
            h.putInt("handle", file.id);
            h.putString("path", file.path);
            h.putLong("position", file.position);
            list.add(h);
        }
        nbt.put("input", list);
    }

    private final class OpenFile implements Handle {
        final int id;
        final String path;
        final byte[] data;
        long position;

        OpenFile(int id, String path, byte[] data) {
            this.id = id;
            this.path = path;
            this.data = data;
        }

        @Override public long position() { return position; }
        @Override public long length() { return data.length; }

        @Override
        public void close() {
            synchronized (FloppyFileSystem.this) {
                handles.remove(id);
            }
        }

        @Override
        public int read(byte[] into) {
            if (position >= data.length) return -1;
            int n = (int) Math.min(into.length, data.length - position);
            System.arraycopy(data, (int) position, into, 0, n);
            position += n;
            return n;
        }

        @Override
        public long seek(long to) {
            position = Math.max(0, Math.min(to, data.length));
            return position;
        }

        @Override
        public void write(byte[] value) {
            throw new RuntimeException(new IOException("bad file descriptor"));
        }
    }
}
