"""Minimal DOM and CSS selector subset for applying parse plans to SERP HTML.

Stdlib only (``html.parser``). This is a reference implementation for the
plan format, not the production parser; ``tenta-search`` (P1) implements the
same selector subset in Rust. Supported selector grammar:

    selector-list := compound (combinator compound)* ("," selector-list)?
    combinator    := " " (descendant) | ">" (child)
    compound      := tag? simple*
    simple        := "#id" | ".class" | "[attr]" | "[attr=val]" | "[attr^=val]"
                   | "[attr$=val]" | "[attr*=val]" | "[attr~=val]"
                   | ":not(compound)" | ":first-child" | ":has(compound)"

Attribute values may be quoted with single or double quotes.
"""

from __future__ import annotations

import re
from html.parser import HTMLParser
from typing import Iterator

VOID_ELEMENTS = frozenset(
    "area base br col embed hr img input link meta param source track wbr".split()
)
_WS = re.compile(r"\s+")


class Node:
    __slots__ = ("tag", "attrs", "children", "parent", "order")

    def __init__(self, tag: str, attrs: dict[str, str], parent: "Node | None", order: int = 0):
        self.tag = tag
        self.attrs = attrs
        self.children: list[Node | str] = []
        self.parent = parent
        self.order = order  # document order, used to sort selector results

    def get(self, name: str, default: str | None = None) -> str | None:
        return self.attrs.get(name, default)

    @property
    def classes(self) -> list[str]:
        return self.attrs.get("class", "").split()

    def iter_elements(self) -> Iterator["Node"]:
        for child in self.children:
            if isinstance(child, Node):
                yield child
                yield from child.iter_elements()

    def element_children(self) -> list["Node"]:
        return [c for c in self.children if isinstance(c, Node)]

    def text(self) -> str:
        parts: list[str] = []
        self._collect_text(parts)
        return _WS.sub(" ", "".join(parts)).strip()

    def _collect_text(self, parts: list[str]) -> None:
        for child in self.children:
            if isinstance(child, str):
                parts.append(child)
            elif child.tag not in ("script", "style"):
                child._collect_text(parts)

    def select(self, selector: str) -> list["Node"]:
        return select(self, selector)

    def select_one(self, selector: str) -> "Node | None":
        found = select(self, selector)
        return found[0] if found else None

    def __repr__(self) -> str:
        ident = ""
        if "id" in self.attrs:
            ident += "#" + self.attrs["id"]
        if self.classes:
            ident += "." + ".".join(self.classes)
        return f"<{self.tag}{ident}>"


class _TreeBuilder(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.root = Node("#document", {}, None)
        self.stack = [self.root]
        self.counter = 0

    def _new_node(self, tag: str, attrs: list[tuple[str, str | None]]) -> Node:
        self.counter += 1
        node = Node(tag, {k: (v or "") for k, v in attrs}, self.stack[-1], self.counter)
        self.stack[-1].children.append(node)
        return node

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        node = self._new_node(tag, attrs)
        if tag not in VOID_ELEMENTS:
            self.stack.append(node)

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self._new_node(tag, attrs)

    def handle_endtag(self, tag: str) -> None:
        for index in range(len(self.stack) - 1, 0, -1):
            if self.stack[index].tag == tag:
                del self.stack[index:]
                return

    def handle_data(self, data: str) -> None:
        if data:
            self.stack[-1].children.append(data)


def parse(html: str) -> Node:
    builder = _TreeBuilder()
    builder.feed(html)
    builder.close()
    return builder.root


# --- selectors -------------------------------------------------------------

_TOKEN = re.compile(
    r"""
    (?P<tag>[a-zA-Z][\w-]*|\*)
  | \#(?P<id>[\w-]+)
  | \.(?P<cls>[\w-]+)
  | \[(?P<attr>[\w-]+)(?:(?P<op>[\^$*~]?=)(?P<val>"[^"]*"|'[^']*'|[^\]]*))?\]
  | :not\((?P<not>[^)]+)\)
  | :has\((?P<has>[^)]+)\)
  | :(?P<pseudo>first-child)
    """,
    re.VERBOSE,
)


class _Simple:
    __slots__ = ("kind", "a", "b", "c")

    def __init__(self, kind: str, a=None, b=None, c=None):
        self.kind, self.a, self.b, self.c = kind, a, b, c

    def matches(self, node: Node) -> bool:
        kind = self.kind
        if kind == "tag":
            return self.a == "*" or node.tag == self.a
        if kind == "id":
            return node.attrs.get("id") == self.a
        if kind == "class":
            return self.a in node.classes
        if kind == "attr":
            value = node.attrs.get(self.a)
            if value is None:
                return False
            op, expected = self.b, self.c
            if op is None:
                return True
            if op == "=":
                return value == expected
            if op == "^=":
                return value.startswith(expected)
            if op == "$=":
                return value.endswith(expected)
            if op == "*=":
                return expected in value
            if op == "~=":
                return expected in value.split()
            return False
        if kind == "not":
            return not self.a.matches(node)
        if kind == "has":
            return any(self.a.matches(descendant) for descendant in node.iter_elements())
        if kind == "first-child":
            parent = node.parent
            return parent is not None and parent.element_children()[0] is node
        raise ValueError(kind)


class _Compound:
    __slots__ = ("simples",)

    def __init__(self, simples: list[_Simple]):
        self.simples = simples

    def matches(self, node: Node) -> bool:
        return all(simple.matches(node) for simple in self.simples)


def _parse_compound(text: str) -> _Compound:
    simples: list[_Simple] = []
    pos = 0
    text = text.strip()
    while pos < len(text):
        match = _TOKEN.match(text, pos)
        if not match:
            raise ValueError(f"bad selector near {text[pos:]!r}")
        pos = match.end()
        if match.group("tag"):
            simples.append(_Simple("tag", match.group("tag").lower()))
        elif match.group("id"):
            simples.append(_Simple("id", match.group("id")))
        elif match.group("cls"):
            simples.append(_Simple("class", match.group("cls")))
        elif match.group("attr"):
            value = match.group("val")
            if value is not None and value[:1] in "\"'":
                value = value[1:-1]
            simples.append(_Simple("attr", match.group("attr").lower(), match.group("op"), value))
        elif match.group("not"):
            simples.append(_Simple("not", _parse_compound(match.group("not"))))
        elif match.group("has"):
            simples.append(_Simple("has", _parse_compound(match.group("has"))))
        elif match.group("pseudo"):
            simples.append(_Simple(match.group("pseudo")))
    if not simples:
        raise ValueError("empty compound selector")
    return _Compound(simples)


def _split_top_level(text: str, separator: str) -> list[str]:
    parts, depth, start = [], 0, 0
    for index, char in enumerate(text):
        if char in "([":
            depth += 1
        elif char in ")]":
            depth -= 1
        elif char == separator and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return [part for part in parts if part.strip()]


def _parse_complex(text: str) -> list[tuple[str, _Compound]]:
    """Return [(combinator, compound)], first combinator is "descendant"."""
    tokens = re.split(r"\s*(>)\s*|\s+", text.strip())
    steps: list[tuple[str, _Compound]] = []
    combinator = " "
    for token in tokens:
        if token is None or token == "":
            continue
        if token == ">":
            combinator = ">"
            continue
        steps.append((combinator, _parse_compound(token)))
        combinator = " "
    return steps


def select(root: Node, selector: str) -> list[Node]:
    results: list[Node] = []
    seen: set[int] = set()
    for alternative in _split_top_level(selector, ","):
        steps = _parse_complex(alternative)
        candidates: list[Node] = [root]
        for combinator, compound in steps:
            next_candidates: list[Node] = []
            picked: set[int] = set()
            for candidate in candidates:
                pool = candidate.iter_elements() if combinator == " " else candidate.element_children()
                for node in pool:
                    if id(node) not in picked and compound.matches(node):
                        picked.add(id(node))
                        next_candidates.append(node)
            candidates = next_candidates
        for node in candidates:
            if id(node) not in seen:
                seen.add(id(node))
                results.append(node)
    results.sort(key=lambda node: node.order)
    return results
