package dev.wackasm.oc.ae2;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import org.junit.jupiter.api.Test;

/**
 * {@link Stacks#list} and {@link Stacks#count}: sequences and counts arrive
 * as Lua tables (keyed 1..n, numbers as doubles) or as WebAssembly arrays
 * (numbers as longs), and both must read the same.
 */
class StacksTest {
    @Test
    void sequencesFromWasmAndLua() {
        assertEquals(List.of("a", "b"), Stacks.list(new Object[] {"a", "b"}, "x"));
        assertEquals(List.of("a", "b"), Stacks.list(List.of("a", "b"), "x"));
        Map<Object, Object> lua = new LinkedHashMap<>();
        lua.put(2.0, "b");
        lua.put(1.0, "a");
        assertEquals(List.of("a", "b"), Stacks.list(lua, "x"));
        assertEquals(List.of(), Stacks.list(Map.of(), "x"));
    }

    @Test
    void notSequences() {
        assertThrows(IllegalArgumentException.class, () -> Stacks.list("a", "x"));
        assertThrows(IllegalArgumentException.class, () -> Stacks.list(Map.of("name", "a"), "x"));
        assertThrows(IllegalArgumentException.class, () -> Stacks.list(Map.of(2.0, "b"), "x")); // a gap
        assertThrows(IllegalArgumentException.class, () -> Stacks.list(Map.of(1.5, "b"), "x"));
    }

    @Test
    void counts() {
        assertEquals(4, Stacks.count(4L));
        assertEquals(4, Stacks.count(4.0));
        assertThrows(IllegalArgumentException.class, () -> Stacks.count(0L));
        assertThrows(IllegalArgumentException.class, () -> Stacks.count(1.5));
        assertThrows(IllegalArgumentException.class, () -> Stacks.count("4"));
        assertThrows(IllegalArgumentException.class, () -> Stacks.count(null));
    }
}
