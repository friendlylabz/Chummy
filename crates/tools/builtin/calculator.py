# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "calculator"
# title = "Calculator"
# version = "1.0.0"
# ///
"""Works out arithmetic exactly, so the model doesn't have to."""

import ast
import math
import operator

import chummy

OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}
FUNCS = {
    "sqrt": math.sqrt, "abs": abs, "round": round, "min": min, "max": max,
    "log": math.log, "log10": math.log10, "exp": math.exp, "floor": math.floor,
    "ceil": math.ceil, "sin": math.sin, "cos": math.cos, "tan": math.tan,
}
NAMES = {"pi": math.pi, "e": math.e}


def ev(node):
    if isinstance(node, ast.Expression):
        return ev(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return node.value
    if isinstance(node, ast.BinOp) and type(node.op) in OPS:
        left, right = ev(node.left), ev(node.right)
        if isinstance(node.op, ast.Pow) and abs(right) > 1000:
            raise chummy.ToolError("That power is too large.")
        return OPS[type(node.op)](left, right)
    if isinstance(node, ast.UnaryOp) and type(node.op) in OPS:
        return OPS[type(node.op)](ev(node.operand))
    if isinstance(node, ast.Name) and node.id in NAMES:
        return NAMES[node.id]
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in FUNCS:
        return FUNCS[node.func.id](*[ev(a) for a in node.args])
    raise chummy.ToolError("Only numbers, + - * / // % **, brackets and sqrt, log, round, min, max, … are allowed.")


@chummy.tool
def calculator(expression: str) -> str:
    """Calculate an arithmetic expression, like (17 * 23) / 4 or sqrt(2).

    expression: the arithmetic to work out
    """
    try:
        tree = ast.parse(expression.replace("^", "**").replace("×", "*"), mode="eval")
    except SyntaxError:
        raise chummy.ToolError(f"That isn't an arithmetic expression: {expression}")
    try:
        value = ev(tree)
    except ZeroDivisionError:
        raise chummy.ToolError("That divides by zero.")
    except (OverflowError, ValueError) as e:
        raise chummy.ToolError(f"Can't calculate that: {e}")
    if isinstance(value, float) and value.is_integer() and abs(value) < 1e15:
        value = int(value)
    return str(value)
