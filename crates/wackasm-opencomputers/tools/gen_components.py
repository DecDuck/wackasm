"""Generates src/components.rs from callbacks.json.

Usage: python gen_components.py callbacks.json ../src/components.rs

Every `@Callback` doc string in OpenComputers has the form
`function(params):returns -- description`, which is enough to produce a
typed Rust method per callback. See the crate README for the type mapping.
"""

import json
import re
import sys

KEYWORDS = {
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
    "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro", "override",
    "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
}
SIGNATURE = re.compile(r"^\s*function\s*\((.*?)\)\s*(?::\s*(.*?))?\s*(?:--\s*(.*))?$", re.S)


def snake(name):
    name = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", name)
    name = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", name)
    return re.sub(r"\W", "_", name).lower()


def ident(name):
    name = snake(name)
    if name in ("self", "super", "crate", "Self"):
        return name + "_"
    return "r#" + name if name in KEYWORDS else name


def pascal(kind):
    return "".join(part[:1].upper() + part[1:] for part in re.split(r"[_\W]+", kind) if part)


def split_top(text):
    """Splits on commas, noting for each piece whether it sits inside [...]."""
    pieces, depth, current, current_depth = [], 0, "", 0
    for ch in text:
        if ch == "[":
            depth += 1
            continue
        if ch == "]":
            depth -= 1
            continue
        if ch == "," and depth >= 0:
            pieces.append((current.strip(), current_depth))
            current, current_depth = "", depth
            continue
        if not current.strip():
            current_depth = depth
        current += ch
    pieces.append((current.strip(), current_depth))
    return [(p, d > 0) for p, d in pieces if p]


class Param:
    def __init__(self, token, optional, index):
        self.optional = optional
        self.varargs = token.endswith("...")
        token = token.rstrip(".")
        match = re.match(r"^(\w+)\s*:\s*(.+?)(?:\s*=\s*(.+))?$", token)
        if match:
            self.name, self.type, self.default = match.group(1), match.group(2).strip(), match.group(3)
        else:
            self.name, self.type, self.default = (token if re.match(r"^\w+$", token) else f"arg{index}"), "any", None

    def rust(self):
        """(declaration type, expression producing Option<Value>)"""
        n = ident(self.name)
        if self.varargs:
            return "&[Value]", None
        t = self.type
        if not self.optional:
            return {
                "number": ("impl Into<Number>", f"Some(Value::Number({n}.into()))"),
                "string": ("impl AsRef<[u8]>", f"Some(Value::text({n}.as_ref()))"),
                "boolean": ("bool", f"Some(Value::Bool({n}))"),
                "userdata": ("Handle", f"Some(Value::Handle({n}))"),
            }.get(t, ("impl Into<Value>", f"Some({n}.into())"))
        return {
            "number": ("Option<Number>", f"{n}.map(Value::Number)"),
            "string": ("Option<&str>", f"{n}.map(Value::from)"),
            "boolean": ("Option<bool>", f"{n}.map(Value::Bool)"),
            "userdata": ("Option<Handle>", f"{n}.map(Value::Handle)"),
        }.get(t, ("Option<Value>", n))


RETURN_TYPES = {"number": "Number", "boolean": "bool", "string": "OcString", "userdata": "Handle"}


def return_type(text):
    text = (text or "").strip()
    if not text or text in ("nil", "void"):
        return "()", "FromReply::from_reply(reply)"
    parts = split_top(text)
    rust = []
    for part, optional in parts:
        part = part.strip()
        m = re.match(r"^(\w+) or nil$", part)
        if m and m.group(1) in RETURN_TYPES:
            rust.append(f"Option<{RETURN_TYPES[m.group(1)]}>")
        elif part in RETURN_TYPES:
            t = RETURN_TYPES[part]
            rust.append(f"Option<{t}>" if optional else t)
        else:
            rust.append("Value")
    if len(rust) == 1:
        return rust[0], f"single::<{rust[0]}>(reply)"
    if len(rust) > 5:
        return "Vec<Value>", "FromReply::from_reply(reply)"
    return f"({', '.join(rust)})", "FromReply::from_reply(reply)"


def doc_lines(text, indent):
    # Square brackets in prose would be read as intra-doc links.
    text = re.sub(r"\s+", " ", text).strip().replace("[", "\\[").replace("]", "\\]")
    return [f"{indent}/// {text}"] if text else []


def method(kind, lua_name, attrs, used):
    match = SIGNATURE.match(attrs["doc"])
    params_text, returns_text, description = match.groups() if match else ("...", "", attrs["doc"])
    params = [Param(tok, opt, i) for i, (tok, opt) in enumerate(split_top(params_text))]
    # Optional parameters can't precede required ones in Rust's call syntax,
    # but OC never does that; mark any stragglers optional to keep order.
    seen_optional = False
    for p in params:
        seen_optional |= p.optional
        p.optional |= seen_optional and not p.varargs
    ret, decode = return_type(returns_text)

    name = ident(lua_name)
    while name in used:
        name += "_"
    used.add(name)

    lines = doc_lines(description or "", "    ")
    lines.append("    ///")
    lines.append(f"    /// Lua: `{kind}.{lua_name}{attrs['doc'].split('--')[0].strip()[len('function'):]}`")
    if attrs.get("direct"):
        limit = attrs.get("limit", 2147483647)
        note = f"; OC's limit {limit}" if limit < 2147483647 else ""
        lines.append(f"    /// Direct call (runs immediately, drawing on the per-tick budget{note}).")
    else:
        lines.append("    /// Indirect call: waits for the next server tick.")
    for p in params:
        if p.optional and p.default:
            lines.append(f"    /// `{ident(p.name)}` defaults to `{p.default.strip()}`.")

    decls, exprs, varargs = [], [], None
    for p in params:
        t, e = p.rust()
        decls.append(f"{ident(p.name)}: {t}")
        if p.varargs:
            varargs = ident(p.name)
        else:
            exprs.append(e)
    sig = ", ".join(["&self", *decls])
    lines.append(f"    pub fn {name}({sig}) -> Result<{ret}> {{")
    if varargs:
        lines.append(f"        let mut args: Vec<Value> = vec![{', '.join(exprs)}].into_iter().map(Option::unwrap_or_default).collect();")
        lines.append(f"        args.extend_from_slice({varargs});")
    else:
        lines.append(f"        let args = args(vec![{', '.join(exprs)}]);")
    lines.append(f"        let reply = self.invoke({json.dumps(lua_name)}, args)?;")
    lines.append(f"        {decode}")
    lines.append("    }")

    # The non-blocking twin, for the `async` feature (see `crate::task`).
    twin = name.removeprefix("r#") + "_async"
    while twin in used:
        twin += "_"
    used.add(twin)
    lines.append("")
    lines.append(f"    /// [`Self::{name.removeprefix('r#')}`] without blocking: the call is queued, and")
    lines.append("    /// the program keeps running until the result comes back; see [`crate::task`].")
    lines.append('    #[cfg(feature = "async")]')
    lines.append(f"    pub async fn {twin}({sig}) -> Result<{ret}> {{")
    if varargs:
        lines.append(f"        let mut args: Vec<Value> = vec![{', '.join(exprs)}].into_iter().map(Option::unwrap_or_default).collect();")
        lines.append(f"        args.extend_from_slice({varargs});")
    else:
        lines.append(f"        let args = args(vec![{', '.join(exprs)}]);")
    lines.append(f"        let reply = self.invoke_async({json.dumps(lua_name)}, args).await?;")
    lines.append(f"        {decode}")
    lines.append("    }")
    return lines


def main(src, dst):
    components = json.load(open(src, encoding="utf8"))
    out = [
        "// @generated by tools/gen_components.py from OpenComputers' @Callback",
        "// annotations (tools/callbacks.json). Do not edit by hand; regenerate.",
        "",
        "//! Typed wrappers for every OpenComputers component.",
        "//!",
        "//! One struct per component type, one method per callback. The docs on",
        "//! each method are OpenComputers' own, followed by the Lua signature.",
        "//! Find components with [`Component::first`] or [`Component::all`].",
        "//! With the `async` feature, each method also has an `_async` twin that",
        "//! doesn't block (see `crate::task`).",
        "",
        "#![allow(clippy::too_many_arguments)]",
        "",
        "use crate::{",
        "    component::{Address, Component, FromReply, args, single},",
        "    error::Result,",
        "    value::{Handle, Number, OcString, Value},",
        "};",
        "",
    ]
    for kind in sorted(components):
        entry = components[kind]
        struct = pascal(kind)
        sources = ", ".join(sorted({c.split(".")[-1] for c in entry["classes"]}))
        out += [
            f"/// The `{kind}` component ({len(entry['methods'])} methods).",
            "///",
            f"/// Provided by OpenComputers' `{sources}`.",
            "#[derive(Clone, Debug, PartialEq, Eq, Hash)]",
            f"pub struct {struct}(pub Address);",
            "",
            f"impl Component for {struct} {{",
            f"    const KIND: &'static str = {json.dumps(kind)};",
            "    fn from_address(address: Address) -> Self {",
            "        Self(address)",
            "    }",
            "    fn address(&self) -> &Address {",
            "        &self.0",
            "    }",
            "}",
            "",
        ]
        if entry["methods"]:
            out.append(f"impl {struct} {{")
            used = set()
            for i, lua_name in enumerate(sorted(entry["methods"])):
                if i:
                    out.append("")
                out += method(kind, lua_name, entry["methods"][lua_name], used)
            out += ["}", ""]
    # Keep the compiler happy when a type is only used by some components.
    out += ["#[allow(dead_code)]", "fn _uses(_: Handle, _: OcString, _: Number) {}", ""]
    with open(dst, "w", encoding="utf8", newline="\n") as f:
        f.write("\n".join(out))
    print(f"wrote {len(components)} components, {sum(len(c['methods']) for c in components.values())} methods to {dst}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
