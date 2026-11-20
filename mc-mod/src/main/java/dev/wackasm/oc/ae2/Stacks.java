package dev.wackasm.oc.ae2;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import appeng.api.stacks.AEFluidKey;
import appeng.api.stacks.AEItemKey;
import appeng.api.stacks.AEKey;
import appeng.api.stacks.GenericStack;
import com.mojang.brigadier.exceptions.CommandSyntaxException;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.TagParser;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.material.Fluids;

/**
 * Converts between AE2 stacks and the tables computers pass around.
 *
 * <p>A stack is a table: {@code {name="minecraft:iron_ingot", count=4}} for
 * items, {@code {name="minecraft:water", count=1000, fluid=true}} for fluids
 * (in millibuckets). Items with data components (enchanted books, named
 * items, ...) can't be named that way; for those, {@code key} carries AE2's
 * own description of the stack as SNBT. Stacks this component returns include
 * it whenever it's needed, so they can be passed back unchanged.
 *
 * <p>Arguments arrive as {@code Map}s keyed 1..n from Lua and as arrays or
 * lists from WebAssembly; {@link #list} accepts all of them.
 */
final class Stacks {
    private Stacks() {}

    /** Reads one stack table. */
    static GenericStack parse(Object value, HolderLookup.Provider registries) {
        if (!(value instanceof Map<?, ?> table)) throw new IllegalArgumentException("a stack must be a table");
        long count = count(table.get("count"));
        Object key = table.get("key");
        if (key != null) {
            AEKey parsed;
            try {
                parsed = AEKey.fromTagGeneric(registries, TagParser.parseTag(string(key, "key")));
            } catch (CommandSyntaxException e) {
                throw new IllegalArgumentException("bad stack key: " + e.getMessage());
            }
            if (parsed == null) throw new IllegalArgumentException("unknown stack key: " + string(key, "key"));
            return new GenericStack(parsed, count);
        }
        String name = string(table.get("name"), "name");
        ResourceLocation id = ResourceLocation.tryParse(name);
        if (id == null) throw new IllegalArgumentException("bad name: " + name);
        if (truthy(table.get("fluid"))) {
            var fluid = BuiltInRegistries.FLUID.getOptional(id)
                    .filter(f -> f != Fluids.EMPTY)
                    .orElseThrow(() -> new IllegalArgumentException("no such fluid: " + name));
            return new GenericStack(AEFluidKey.of(fluid), count);
        }
        var item = BuiltInRegistries.ITEM.getOptional(id)
                .filter(i -> i != Items.AIR)
                .orElseThrow(() -> new IllegalArgumentException("no such item: " + name));
        return new GenericStack(AEItemKey.of(item), count);
    }

    /** Reads a sequence of stack tables; equal stacks are not merged. */
    static List<GenericStack> parseList(Object value, HolderLookup.Provider registries, String what) {
        List<Object> items = list(value, what);
        List<GenericStack> stacks = new ArrayList<>(items.size());
        for (Object item : items) stacks.add(parse(item, registries));
        return stacks;
    }

    /** One stack as a table. */
    static Map<String, Object> describe(AEKey key, long count, HolderLookup.Provider registries) {
        Map<String, Object> table = new LinkedHashMap<>();
        table.put("name", key.getId().toString());
        table.put("count", count);
        table.put("fluid", key instanceof AEFluidKey);
        table.put("label", key.getDisplayName().getString());
        if (key.hasComponents()) table.put("key", key.toTagGeneric(registries).toString());
        return table;
    }

    static Object[] describe(List<GenericStack> stacks, HolderLookup.Provider registries) {
        return stacks.stream().map(s -> describe(s.what(), s.amount(), registries)).toArray();
    }

    /** The elements of a sequence: an array, a list, or a table keyed 1..n. */
    static List<Object> list(Object value, String what) {
        if (value instanceof Object[] array) return Arrays.asList(array);
        if (value instanceof List<?> list) return new ArrayList<>(list);
        if (value instanceof Map<?, ?> map) {
            Object[] items = new Object[map.size()];
            for (Map.Entry<?, ?> e : map.entrySet()) {
                int index = e.getKey() instanceof Number n && n.doubleValue() == Math.rint(n.doubleValue())
                        ? n.intValue() - 1 : -1;
                if (index < 0 || index >= items.length || items[index] != null) {
                    throw new IllegalArgumentException(what + " must be a sequence");
                }
                items[index] = e.getValue();
            }
            return Arrays.asList(items);
        }
        throw new IllegalArgumentException(what + " must be a sequence");
    }

    static long count(Object value) {
        if (!(value instanceof Number n)) throw new IllegalArgumentException("count must be a number");
        double d = n.doubleValue();
        if (d != Math.rint(d) || d < 1 || d > Long.MAX_VALUE) {
            throw new IllegalArgumentException("count must be a positive whole number");
        }
        return n.longValue();
    }

    private static String string(Object value, String what) {
        if (value instanceof String s) return s;
        if (value instanceof byte[] b) return new String(b, StandardCharsets.UTF_8);
        throw new IllegalArgumentException(what + " must be a string");
    }

    private static boolean truthy(Object value) {
        return value != null && !Boolean.FALSE.equals(value);
    }

    // NBT for saving; AE2's own format, so stacks survive any item changes AE2 survives.

    static CompoundTag save(GenericStack stack, HolderLookup.Provider registries) {
        return GenericStack.writeTag(registries, stack);
    }

    static GenericStack load(CompoundTag tag, HolderLookup.Provider registries) {
        return GenericStack.readTag(registries, tag);
    }
}
