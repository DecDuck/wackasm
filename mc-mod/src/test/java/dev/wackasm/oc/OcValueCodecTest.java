package dev.wackasm.oc;

import static org.junit.jupiter.api.Assertions.*;

import java.util.LinkedHashMap;
import java.util.Map;

import li.cil.oc.api.machine.Arguments;
import li.cil.oc.api.machine.Context;
import li.cil.oc.api.machine.Value;
import net.minecraft.nbt.NBTTagCompound;
import org.junit.jupiter.api.Test;

class OcValueCodecTest {
    /** Stand-in for OC userdata such as HandleValue. */
    public static final class FakeHandle implements Value {
        int handle;
        boolean disposed;

        public FakeHandle() {}

        FakeHandle(int handle) {
            this.handle = handle;
        }

        @Override public Object apply(Context c, Arguments a) { return null; }
        @Override public void unapply(Context c, Arguments a) {}
        @Override public Object[] call(Context c, Arguments a) { return null; }
        @Override public void dispose(Context c) { disposed = true; }
        @Override public void load(NBTTagCompound nbt) { handle = nbt.getInteger("h"); }
        @Override public void save(NBTTagCompound nbt) { nbt.setInteger("h", handle); }
    }

    /**
     * The same bytes are asserted in crates/wackasm-oc/src/value.rs
     * (`matches_java_vector`), which pins the two codecs together.
     */
    static final byte[] VECTOR = {
        7, 5, 0, 0, 0,          // array of 5
        0,                      // nil
        2,                      // true
        3, 7, 0, 0, 0, 0, 0, 0, 0, // int 7
        5, 2, 0, 0, 0, 'h', 'i',   // "hi"
        9, 1, 0, 0, 0,          // userdata #1
    };

    @Test
    void encodesTheSharedVector() {
        UserdataTable table = new UserdataTable();
        byte[] bytes = OcValueCodec.encode(new Object[] {null, true, 7, "hi", new FakeHandle(42)}, table);
        assertArrayEquals(VECTOR, bytes);
    }

    @Test
    void roundTripsEveryType() {
        UserdataTable table = new UserdataTable();
        FakeHandle handle = new FakeHandle(42);
        Map<Object, Object> map = new LinkedHashMap<>();
        map.put("k", 1L);
        Object[] in = {null, true, false, 7L, 1.5, "hé", new byte[] {1, 2}, new Object[] {"x"}, map, handle, handle};
        Object[] out = OcValueCodec.decodeArray(OcValueCodec.encode(in, table), table);

        assertNull(out[0]);
        assertEquals(true, out[1]);
        assertEquals(false, out[2]);
        assertEquals(7L, out[3]);
        assertEquals(1.5, out[4]);
        assertEquals("hé", out[5]);
        assertArrayEquals(new byte[] {1, 2}, (byte[]) out[6]);
        assertArrayEquals(new Object[] {"x"}, (Object[]) out[7]);
        assertEquals(map, out[8]);
        // Userdata comes back as the very same object, under one id.
        assertSame(handle, out[9]);
        assertSame(handle, out[10]);
    }

    @Test
    void unknownUserdataIsNil() {
        assertNull(OcValueCodec.decode(new byte[] {9, 99, 0, 0, 0}, new UserdataTable()));
    }

    @Test
    void rejectsMalformed() {
        UserdataTable table = new UserdataTable();
        assertThrows(RuntimeException.class, () -> OcValueCodec.decode(new byte[] {3, 1}, table));
        assertThrows(RuntimeException.class, () -> OcValueCodec.decode(new byte[] {0, 0}, table));
        assertThrows(RuntimeException.class, () -> OcValueCodec.decode(new byte[] {7, -1, -1, -1, 127}, table));
    }

    @Test
    void userdataSurvivesNbtAndIsDisposed() {
        UserdataTable table = new UserdataTable();
        FakeHandle handle = new FakeHandle(42);
        int id = table.idOf(handle);
        NBTTagCompound nbt = new NBTTagCompound();
        table.save(nbt);

        UserdataTable restored = new UserdataTable();
        restored.load(nbt);
        FakeHandle back = (FakeHandle) restored.get(id);
        assertEquals(42, back.handle);
        // New ids don't collide with restored ones.
        assertNotEquals(id, restored.idOf(new FakeHandle(1)));

        restored.disposeAll(null);
        assertTrue(back.disposed);
        assertNull(restored.get(id));
    }
}
