"""Bounded interpreter for the small Python repair task. Never exec model code."""
import ast
import math
import operator

NUMERIC = (int, float)
BINARY = {ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul, ast.Div: operator.truediv}
COMPARE = {ast.Eq: operator.eq, ast.NotEq: operator.ne, ast.Lt: operator.lt,
           ast.LtE: operator.le, ast.Gt: operator.gt, ast.GtE: operator.ge}


def number(value):
    if type(value) not in NUMERIC or not math.isfinite(value) or abs(value) > 1e9:
        raise ValueError('numeric value outside fixture bounds')
    return value


def expression(node, values, budget):
    budget[0] -= 1
    if budget[0] < 0:
        raise ValueError('fixture expression budget exceeded')
    if isinstance(node, ast.Constant):
        return number(node.value)
    if isinstance(node, ast.Name) and node.id == 'values':
        return values
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in ('len', 'sum') and len(node.args) == 1 and not node.keywords:
        arg = expression(node.args[0], values, budget)
        if not isinstance(arg, list):
            raise ValueError('len/sum requires the input list')
        return number((len if node.func.id == 'len' else sum)(arg))
    if isinstance(node, ast.BinOp) and type(node.op) in BINARY:
        left = number(expression(node.left, values, budget))
        right = number(expression(node.right, values, budget))
        return number(BINARY[type(node.op)](left, right))
    if isinstance(node, ast.UnaryOp):
        value = expression(node.operand, values, budget)
        if isinstance(node.op, ast.Not):
            return not value
        if isinstance(node.op, ast.USub):
            return number(-number(value))
    if isinstance(node, ast.IfExp):
        branch = node.body if expression(node.test, values, budget) else node.orelse
        return expression(branch, values, budget)
    if isinstance(node, ast.Compare):
        left = expression(node.left, values, budget)
        for op, comparator in zip(node.ops, node.comparators):
            if type(op) not in COMPARE:
                raise ValueError('unsupported comparison')
            right = expression(comparator, values, budget)
            if not COMPARE[type(op)](left, right):
                return False
            left = right
        return True
    raise ValueError('unsupported expression in bounded function fixture')


def statements(nodes, values, budget):
    for node in nodes:
        budget[0] -= 1
        if budget[0] < 0:
            raise ValueError('fixture statement budget exceeded')
        if isinstance(node, ast.Return):
            return True, expression(node.value, values, budget)
        if isinstance(node, ast.If):
            branch = node.body if expression(node.test, values, budget) else node.orelse
            returned, result = statements(branch, values, budget)
            if returned:
                return True, result
        elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            continue  # A docstring is data, never executed.
        else:
            raise ValueError('unsupported statement in bounded function fixture')
    return False, None


def check_python(source):
    cases = [([], 0.0), ([1, 2, 3], 2), ([-3, 1], -1), ([0], 0), ([1, 2], 1.5)]
    try:
        if len(source) > 8192:
            raise ValueError('source exceeds fixture bound')
        tree = ast.parse(source)
        if len(list(ast.walk(tree))) > 128:
            raise ValueError('source exceeds AST node bound')
        if len(tree.body) != 1 or not isinstance(tree.body[0], ast.FunctionDef) or tree.body[0].name != 'mean':
            raise ValueError('expected exactly one mean function')
        fn = tree.body[0]
        a = fn.args
        if fn.decorator_list or fn.returns or len(a.args) != 1 or a.args[0].arg != 'values' or a.args[0].annotation or a.defaults or a.vararg or a.kwarg or a.kwonlyargs or a.posonlyargs or getattr(fn, "type_params", []):
            raise ValueError('expected mean(values) without annotations or defaults')
        outcomes = []
        for values, expected in cases:
            try:
                returned, result = statements(fn.body, values, [256])
                outcomes.append(returned and type(result) in NUMERIC and result == expected)
            except Exception:
                outcomes.append(False)
        return {'passed': sum(outcomes), 'total': len(cases)}
    except Exception as error:
        return {'passed': 0, 'total': len(cases), 'error': str(error)}
