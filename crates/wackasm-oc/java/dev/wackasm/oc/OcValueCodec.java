package dev.wackasm.oc;

import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Java twin of {@code crates/wackasm-oc/src/value.rs}. Keep the two in sync.
 *
 * <p>Decoding produces {@code null}, {@link Boolean}, {@link Long}, {@link Double},
 * {@link String}, {@code byte[]}, {@code Object[]} and {@code Map<Object, Object>}.
 * OpenComputers' {@code Arguments} accepts all of these. When encoding, any
 * other object (such as userdata {@code Value}s) is sent as its {@code toString()}.
 */
public final class OcValueCodec {
    private static final int TAG_NIL = 0, TAG_FALSE = 1, TAG_TRUE = 2, TAG_INT = 3, TAG_FLOAT = 4,
            TAG_STRING = 5, TAG_BYTES = 6, TAG_ARRAY = 7, TAG_MAP = 8;
    private static final int MAX_DEPTH = 64;

    private OcValueCodec() {}

    public static byte[] encode(Object value) {
        Writer w = new Writer();
        w.value(value, 0);
        return w.out.toByteArray();
    }

    /** Encodes {@code values} as one array value. */
    public static byte[] encodeArray(Object... values) {
        return encode(values);
    }

    public static Object decode(byte[] bytes) {
        ByteBuffer buf = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        Object value = read(buf, 0);
        if (buf.hasRemaining()) throw new IllegalArgumentException("trailing bytes after value");
        return value;
    }

    public static Object[] decodeArray(byte[] bytes) {
        Object value = decode(bytes);
        if (!(value instanceof Object[])) throw new IllegalArgumentException("expected an array");
        return (Object[]) value;
    }

    private static Object read(ByteBuffer buf, int depth) {
        if (depth > MAX_DEPTH) throw new IllegalArgumentException("value nested too deeply");
        int tag = buf.get() & 0xff;
        switch (tag) {
            case TAG_NIL: return null;
            case TAG_FALSE: return Boolean.FALSE;
            case TAG_TRUE: return Boolean.TRUE;
            case TAG_INT: return buf.getLong();
            case TAG_FLOAT: return buf.getDouble();
            case TAG_STRING: return new String(bytes(buf), StandardCharsets.UTF_8);
            case TAG_BYTES: return bytes(buf);
            case TAG_ARRAY: {
                Object[] items = new Object[count(buf, 1)];
                for (int i = 0; i < items.length; i++) items[i] = read(buf, depth + 1);
                return items;
            }
            case TAG_MAP: {
                int n = count(buf, 2);
                Map<Object, Object> map = new LinkedHashMap<>();
                for (int i = 0; i < n; i++) map.put(read(buf, depth + 1), read(buf, depth + 1));
                return map;
            }
            default: throw new IllegalArgumentException("unknown value tag " + tag);
        }
    }

    private static int count(ByteBuffer buf, int minBytesEach) {
        long n = buf.getInt() & 0xffffffffL;
        if (n * minBytesEach > buf.remaining()) throw new IllegalArgumentException("count exceeds input");
        return (int) n;
    }

    private static byte[] bytes(ByteBuffer buf) {
        byte[] b = new byte[count(buf, 1)];
        buf.get(b);
        return b;
    }

    private static final class Writer {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        private final ByteBuffer scratch = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN);

        void value(Object v, int depth) {
            if (depth > MAX_DEPTH) throw new IllegalArgumentException("value nested too deeply");
            if (v == null) {
                out.write(TAG_NIL);
            } else if (v instanceof Boolean) {
                out.write((Boolean) v ? TAG_TRUE : TAG_FALSE);
            } else if (v instanceof Byte || v instanceof Short || v instanceof Integer || v instanceof Long) {
                out.write(TAG_INT);
                scratch.clear();
                out.write(scratch.putLong(((Number) v).longValue()).array(), 0, 8);
            } else if (v instanceof Number) {
                out.write(TAG_FLOAT);
                scratch.clear();
                out.write(scratch.putDouble(((Number) v).doubleValue()).array(), 0, 8);
            } else if (v instanceof byte[]) {
                blob(TAG_BYTES, (byte[]) v);
            } else if (v instanceof Object[]) {
                Object[] items = (Object[]) v;
                header(TAG_ARRAY, items.length);
                for (Object item : items) value(item, depth + 1);
            } else if (v instanceof Collection) {
                Collection<?> items = (Collection<?>) v;
                header(TAG_ARRAY, items.size());
                for (Object item : items) value(item, depth + 1);
            } else if (v instanceof Map) {
                Map<?, ?> map = (Map<?, ?>) v;
                header(TAG_MAP, map.size());
                for (Map.Entry<?, ?> e : map.entrySet()) {
                    value(e.getKey(), depth + 1);
                    value(e.getValue(), depth + 1);
                }
            } else {
                blob(TAG_STRING, v.toString().getBytes(StandardCharsets.UTF_8));
            }
        }

        private void header(int tag, int len) {
            out.write(tag);
            scratch.clear();
            out.write(scratch.putInt(len).array(), 0, 4);
        }

        private void blob(int tag, byte[] b) {
            header(tag, b.length);
            out.write(b, 0, b.length);
        }
    }
}
