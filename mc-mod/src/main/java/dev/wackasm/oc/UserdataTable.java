package dev.wackasm.oc;

import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.Map;

import li.cil.oc.api.machine.Context;
import li.cil.oc.api.machine.Value;
import net.minecraft.nbt.NBTTagCompound;
import net.minecraft.nbt.NBTTagList;

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

    void save(NBTTagCompound nbt) {
        NBTTagList list = new NBTTagList();
        for (Map.Entry<Integer, Value> e : byId.entrySet()) {
            NBTTagCompound entry = new NBTTagCompound();
            entry.setInteger("id", e.getKey());
            entry.setString("class", e.getValue().getClass().getName());
            NBTTagCompound data = new NBTTagCompound();
            e.getValue().save(data);
            entry.setTag("data", data);
            list.appendTag(entry);
        }
        nbt.setTag("values", list);
        nbt.setInteger("nextId", nextId);
    }

    void load(NBTTagCompound nbt) {
        byId.clear();
        byValue.clear();
        NBTTagList list = nbt.getTagList("values", 10 /* compound */);
        for (int i = 0; i < list.tagCount(); i++) {
            NBTTagCompound entry = list.getCompoundTagAt(i);
            try {
                Value value = (Value) Class.forName(entry.getString("class")).newInstance();
                value.load(entry.getCompoundTag("data"));
                byId.put(entry.getInteger("id"), value);
                byValue.put(value, entry.getInteger("id"));
            } catch (ReflectiveOperationException | ClassCastException e) {
                // The mod that provided it is gone. The guest will see nil for this id.
                if (WackasmMod.log != null) WackasmMod.log.warn("Dropping unrestorable userdata {}", entry, e);
            }
        }
        nextId = Math.max(1, nbt.getInteger("nextId"));
    }
}
