"""Small helpers shared by the template generators (make_templates.py, assistant2.py)."""


def b(id, type, label, x, y, params=None, comment=""):
    out = {"id": id, "type": type, "label": label, "position": {"x": x, "y": y}, "params": params or {}}
    if comment:
        out["comment"] = comment
    return out


def l(a, ap, c, cp, label=""):
    out = {"from": {"block": a, "port": ap}, "to": {"block": c, "port": cp}}
    if label:
        out["label"] = label
    return out


def note(id, x, y, text, w=320, h=0):
    return {"id": id, "text": text, "position": {"x": x, "y": y}, "width": w, "height": h}


def fn(id, name, description, inputs, outputs, blocks, links, notes=None):
    out = {
        "id": id,
        "name": name,
        "description": description,
        "inputs": [{"name": n, "label": lab, "to": {"block": blk, "port": p}} for n, lab, blk, p in inputs],
        "outputs": [{"name": n, "label": lab, "from": {"block": blk, "port": p}} for n, lab, blk, p in outputs],
        "blocks": blocks,
        "links": links,
    }
    if notes:
        out["notes"] = notes
    return out


def tidy(loop):
    """Model blocks that do the work inherit the agent's tools and every model setting;
    the others (judges, checks, plans, notes) never get tools."""
    def walk(blocks):
        for blk in blocks:
            if blk["type"] != "llm":
                continue
            p = blk.setdefault("params", {})
            p.pop("max_tokens", None)
            if blk["id"] != "work":
                p["tools"] = "off"
    walk(loop["blocks"])
    for f in loop.get("functions", []):
        walk(f["blocks"])
    return loop
