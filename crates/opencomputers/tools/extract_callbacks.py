"""Extracts every OpenComputers component's callbacks from the mod jar.

Usage: python extract_callbacks.py <OpenComputers.jar> <out.json>

For each class it reads the `@Callback` annotations on its methods and the
string constants passed to `withComponent(...)` (the component's name), then
resolves inherited callbacks through superclasses and interfaces (Scala traits
carry many of them). The output maps component name -> method -> callback
attributes, and feeds gen_components.py.
"""

import json
import struct
import sys
import zipfile

# Packages with OpenComputers' own components (blocks, cards, upgrades, and
# the vanilla blocks the Adapter exposes). Other mods' integrations are left out.
PACKAGES = (
    "li/cil/oc/server/component/",
    "li/cil/oc/server/machine/",
    "li/cil/oc/common/tileentity/",
    "li/cil/oc/common/item/",
    "li/cil/oc/integration/opencomputers/",
    "li/cil/oc/integration/minecraft/",
    "li/cil/oc/integration/vanilla/",
    "li/cil/oc/server/fs/",
    "li/cil/oc/common/component/",
)

# Bytes following each opcode, for walking bytecode. -1: variable length.
OPERAND_BYTES = {
    **{op: 0 for op in range(0x00, 0x10)}, 0x10: 1, 0x11: 2, 0x12: 1, 0x13: 2, 0x14: 2,
    **{op: 1 for op in range(0x15, 0x1a)}, **{op: 0 for op in range(0x1a, 0x36)},
    **{op: 1 for op in range(0x36, 0x3b)}, **{op: 0 for op in range(0x3b, 0x84)},
    0x84: 2, **{op: 0 for op in range(0x85, 0x99)}, **{op: 2 for op in range(0x99, 0xa9)},
    0xa9: 1, 0xaa: -1, 0xab: -1, **{op: 0 for op in range(0xac, 0xb2)},
    **{op: 2 for op in range(0xb2, 0xb9)}, 0xb9: 4, 0xba: 4, 0xbb: 2, 0xbc: 1, 0xbd: 2,
    0xbe: 0, 0xbf: 0, 0xc0: 2, 0xc1: 2, 0xc2: 0, 0xc3: 0, 0xc4: -1, 0xc5: 3, 0xc6: 2,
    0xc7: 2, 0xc8: 4, 0xc9: 4,
}


class ClassFile:
    def __init__(self, data):
        self.data = data
        self.pos = 8
        self.pool = [None]
        count = self.u2()
        i = 1
        while i < count:
            tag = self.u1()
            if tag == 1:
                n = self.u2()
                self.pool.append(("utf8", self.data[self.pos:self.pos + n].decode("utf8", "replace")))
                self.pos += n
            elif tag in (3, 4):
                self.pool.append(("num", self.u4()))
            elif tag in (5, 6):
                self.pos += 8
                self.pool += [("wide", None), None]
                i += 1
            elif tag in (7, 8, 16, 19, 20):
                self.pool.append(({7: "class", 8: "string"}.get(tag, "other"), self.u2()))
            elif tag in (9, 10, 11, 12, 17, 18):
                self.pool.append(({12: "nat"}.get(tag, "ref"), (self.u2(), self.u2())))
            elif tag == 15:
                self.pos += 3
                self.pool.append(("other", None))
            else:
                raise ValueError(f"bad constant tag {tag}")
            i += 1
        self.access = self.u2()
        self.name = self.class_name(self.u2())
        super_index = self.u2()
        self.super = self.class_name(super_index) if super_index else None
        self.interfaces = [self.class_name(self.u2()) for _ in range(self.u2())]
        for _ in range(self.u2()):  # fields
            self.pos += 6
            self.skip_attributes()
        self.callbacks = {}
        self.component_names = set()
        for _ in range(self.u2()):
            self.pos += 2
            name = self.utf8(self.u2())
            self.u2()
            for _ in range(self.u2()):
                attr = self.utf8(self.u2())
                length = self.u4()
                end = self.pos + length
                if attr == "RuntimeVisibleAnnotations":
                    self.annotations(name)
                elif attr == "Code":
                    self.code()
                self.pos = end

    def u1(self):
        self.pos += 1
        return self.data[self.pos - 1]

    def u2(self):
        self.pos += 2
        return struct.unpack(">H", self.data[self.pos - 2:self.pos])[0]

    def u4(self):
        self.pos += 4
        return struct.unpack(">I", self.data[self.pos - 4:self.pos])[0]

    def utf8(self, index):
        return self.pool[index][1]

    def class_name(self, index):
        return self.utf8(self.pool[index][1])

    def skip_attributes(self):
        for _ in range(self.u2()):
            self.pos += 2
            length = self.u4()  # not `pos += u4()`: that reads pos before u4 advances it
            self.pos += length

    def element_value(self):
        tag = chr(self.u1())
        if tag in "BCDFIJSZs":
            index = self.u2()
            entry = self.pool[index]
            if tag == "s":
                return entry[1]
            if tag == "Z":
                return bool(entry[1])
            return entry[1]
        if tag == "e":
            self.pos += 4
            return None
        if tag == "c":
            self.pos += 2
            return None
        if tag == "@":
            self.annotation()
            return None
        if tag == "[":
            return [self.element_value() for _ in range(self.u2())]
        raise ValueError(f"bad element tag {tag}")

    def annotation(self):
        kind = self.utf8(self.u2())
        values = {}
        for _ in range(self.u2()):
            key = self.utf8(self.u2())
            values[key] = self.element_value()
        return kind, values

    def annotations(self, method):
        for _ in range(self.u2()):
            kind, values = self.annotation()
            if kind == "Lli/cil/oc/api/machine/Callback;":
                self.callbacks[values.get("value") or method] = {
                    "doc": values.get("doc", ""),
                    "direct": values.get("direct", False),
                    "limit": values.get("limit", 2147483647),
                    "getter": values.get("getter", False),
                    "setter": values.get("setter", False),
                }

    def ref_name(self, index):
        entry = self.pool[index]
        if entry and entry[0] == "ref":
            nat = self.pool[entry[1][1]]
            return self.utf8(nat[1][0])
        return None

    def ref_owner(self, index):
        entry = self.pool[index]
        if entry and entry[0] == "ref":
            return self.class_name(entry[1][0])
        return None

    def code(self):
        self.pos += 4
        length = self.u4()
        start = self.pos
        code = self.data[start:start + length]
        last_string = None
        pc = 0
        while pc < len(code):
            op = code[pc]
            size = OPERAND_BYTES.get(op, 0)
            if op in (0x12, 0x13):  # ldc, ldc_w
                index = code[pc + 1] if op == 0x12 else struct.unpack(">H", code[pc + 1:pc + 3])[0]
                entry = self.pool[index]
                last_string = self.utf8(entry[1]) if entry and entry[0] == "string" else last_string
            elif op in (0xb6, 0xb7, 0xb8, 0xb9):
                index = struct.unpack(">H", code[pc + 1:pc + 3])[0]
                if self.ref_name(index) == "withComponent" and last_string:
                    self.component_names.add(last_string)
                # The Adapter's vanilla blocks: super(tileEntity, "furnace").
                elif (op == 0xb7 and self.ref_name(index) == "<init>" and last_string
                      and (self.ref_owner(index) or "").endswith("/ManagedTileEntityEnvironment")):
                    self.component_names.add(last_string)
            if size == -1:
                if op == 0xc4:  # wide
                    size = 5 if code[pc + 1] == 0x84 else 3
                else:
                    pad = (4 - (pc + 1) % 4) % 4
                    base = pc + 1 + pad
                    if op == 0xaa:
                        low, high = struct.unpack(">ii", code[base + 4:base + 12])
                        size = pad + 12 + 4 * (high - low + 1)
                    else:
                        pairs = struct.unpack(">i", code[base + 4:base + 8])[0]
                        size = pad + 8 + 8 * pairs
            pc += 1 + size
        self.pos = start + length


def main(jar_path, out_path):
    classes = {}
    with zipfile.ZipFile(jar_path) as jar:
        for info in jar.infolist():
            if info.filename.startswith("li/cil/oc/") and info.filename.endswith(".class"):
                cls = ClassFile(jar.read(info))
                classes[cls.name] = cls

    def all_callbacks(name, seen=None):
        seen = seen if seen is not None else set()
        if name in seen or name not in classes:
            return {}
        seen.add(name)
        cls = classes[name]
        merged = {}
        for parent in [cls.super, *cls.interfaces]:
            if parent:
                merged.update(all_callbacks(parent, seen))
        merged.update(cls.callbacks)
        return merged

    # A Scala trait's node is built in its `Trait$class` impl: credit the trait.
    for name, cls in list(classes.items()):
        if name.endswith("$class") and cls.component_names and name[:-6] in classes:
            classes[name[:-6]].component_names |= cls.component_names

    # Names flow down: tier subclasses (DataCard$Tier1, Redstone$Vanilla...)
    # take their name from the base class or trait that builds the node.
    def all_names(name, seen=None):
        seen = seen if seen is not None else set()
        if name in seen or name not in classes:
            return set()
        seen.add(name)
        cls = classes[name]
        names = set(cls.component_names)
        for parent in [cls.super, *cls.interfaces]:
            if parent:
                names |= all_names(parent, seen)
        return names

    components = {}
    for name, cls in sorted(classes.items()):
        if not name.startswith(PACKAGES) or name.endswith("$class"):
            continue
        names = all_names(name)
        if not names:
            continue
        callbacks = all_callbacks(name)
        for component in names:
            entry = components.setdefault(component, {"classes": [], "methods": {}})
            entry["classes"].append(name.replace("/", "."))
            for method, attrs in callbacks.items():
                entry["methods"].setdefault(method, attrs)

    with open(out_path, "w", encoding="utf8") as out:
        json.dump(components, out, indent=1, sort_keys=True)
    total = sum(len(c["methods"]) for c in components.values())
    print(f"{len(components)} components, {total} methods")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
