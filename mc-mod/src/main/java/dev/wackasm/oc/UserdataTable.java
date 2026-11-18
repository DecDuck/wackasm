package dev.wackasm.oc;

import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.Map;

import li.cil.oc.api.machine.Context;
import li.cil.oc.api.machine.Value;
import net.minecraft.core.HolderLookup;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.Tag;

/**
 * The host objects a guest holds, by the ids that cross the codec as
 * userdata, for example the {@code HandleValue} that {@code filesystem.open}
 * returns.
 *
 * <p>The wasm side only ever sees ids, so objects stay referenced until the
 * machine stops. Then they are disposed of, as Lua's garbage collector would.
 * They are persisted with the machine in the same way OpenComputers persists
 * Lua userdata: class name plus {@link Value#save}, rebuilt through the public
 * no-argument constructor that OC requires every {@code Value} to have.
 */
final class UserdataTable {
    private final Map<Integer, Value> byId = new LinkedHashMap<>();
    private final Map<Value, Integer> byValue = new IdentityHashMap<>();
    private int nextId = 1;

    int idOf(Value value) {
        Integer id = byValue.get(value);
        if (id == null) {
            id = nextId++;
            byId.put(id, value);
            byValue.put(value, id);
        }
        return id;
    }

    /** Null for an id that was never handed out, which the guest sees as nil. */
    Value get(int id) {
        return byId.get(id);
    }

    /** Disposes of one value the guest is done with, and forgets its id. */
    void release(Value value, Context context) {
        Integer id = byValue.remove(value);
        if (id == null) return;
        byId.remove(id);
        try {
            value.dispose(context);
        } catch (Throwable ignored) {
            // Releasing must not crash the machine.
        }
    }

    void disposeAll(Context context) {
        for (Value value : byId.values()) {
            try {
                value.dispose(context);
            } catch (Throwable ignored) {
                // A failing dispose must not stop the others.
            }
        }
        byId.clear();
        byValue.clear();
        nextId = 1;
    }

    void save(CompoundTag nbt, HolderLookup.Provider registries) {
        ListTag list = new ListTag();
        for (Map.Entry<Integer, Value> e : byId.entrySet()) {
            CompoundTag entry = new CompoundTag();
            entry.putInt("id", e.getKey());
            entry.putString("class", e.getValue().getClass().getName());
            CompoundTag data = new CompoundTag();
            e.getValue().save(data, registries);
            entry.put("data", data);
            list.add(entry);
        }
        nbt.put("values", list);
        nbt.putInt("nextId", nextId);
    }

    void load(CompoundTag nbt, HolderLookup.Provider registries) {
        byId.clear();
        byValue.clear();
        ListTag list = nbt.getList("values", Tag.TAG_COMPOUND);
        for (int i = 0; i < list.size(); i++) {
            CompoundTag entry = list.getCompound(i);
            try {
                Value value = (Value) Class.forName(entry.getString("class")).getDeclaredConstructor().newInstance();
                value.load(entry.getCompound("data"), registries);
                byId.put(entry.getInt("id"), value);
                byValue.put(value, entry.getInt("id"));
            } catch (ReflectiveOperationException | ClassCastException e) {
                // The mod that provided it is gone. The guest will see nil for this id.
                WackasmMod.log.warn("Dropping unrestorable userdata {}", entry, e);
            }
        }
        nextId = Math.max(1, nbt.getInt("nextId"));
    }
}
