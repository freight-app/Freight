const std = @import("std");
const print = std.debug.print;

// ── Comptime generic stack ────────────────────────────────────────────────────

fn Stack(comptime T: type) type {
    return struct {
        const Self = @This();
        items: [64]T = undefined,
        len: usize = 0,

        fn push(self: *Self, v: T) !void {
            if (self.len >= self.items.len) return error.Overflow;
            self.items[self.len] = v;
            self.len += 1;
        }

        fn pop(self: *Self) !T {
            if (self.len == 0) return error.Empty;
            self.len -= 1;
            return self.items[self.len];
        }

        fn peek(self: *const Self) !T {
            if (self.len == 0) return error.Empty;
            return self.items[self.len - 1];
        }
    };
}

// ── Tagged union ──────────────────────────────────────────────────────────────

const Expr = union(enum) {
    num: f64,
    add: struct { a: *const Expr, b: *const Expr },
    mul: struct { a: *const Expr, b: *const Expr },

    fn eval(self: Expr) f64 {
        return switch (self) {
            .num => |v| v,
            .add => |p| p.a.eval() + p.b.eval(),
            .mul => |p| p.a.eval() * p.b.eval(),
        };
    }
};

// ── Comptime Fibonacci ────────────────────────────────────────────────────────

fn fib(comptime n: u32) u64 {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}

// ── Error handling ────────────────────────────────────────────────────────────

fn parseInt(s: []const u8) !i64 {
    return std.fmt.parseInt(i64, s, 10);
}

fn safeDivide(a: i64, b: i64) !i64 {
    if (b == 0) return error.DivisionByZero;
    return @divTrunc(a, b);
}

// ── Main ──────────────────────────────────────────────────────────────────────

pub fn main() !void {
    // Comptime generic stack
    print("─── Comptime generic Stack(i32) ───\n", .{});
    var s = Stack(i32){};
    try s.push(10);
    try s.push(20);
    try s.push(30);
    print("peek: {d}\n", .{try s.peek()});
    print("pop:  {d}\n", .{try s.pop()});
    print("pop:  {d}\n", .{try s.pop()});

    // Tagged union expression tree: (3 + 4) * 2
    print("\n─── Tagged union expression tree ───\n", .{});
    const three = Expr{ .num = 3 };
    const four  = Expr{ .num = 4 };
    const two   = Expr{ .num = 2 };
    const sum   = Expr{ .add = .{ .a = &three, .b = &four } };
    const prod  = Expr{ .mul = .{ .a = &sum,   .b = &two  } };
    print("(3 + 4) * 2 = {d}\n", .{prod.eval()});

    // Comptime Fibonacci (evaluated at compile time)
    print("\n─── Comptime Fibonacci ───\n", .{});
    print("fib(10) = {d}  (computed at compile time)\n", .{fib(10)});
    print("fib(20) = {d}\n", .{fib(20)});

    // Error handling with try/catch
    print("\n─── Error handling ───\n", .{});
    const inputs = [_][]const u8{ "42", "bad", "100" };
    for (inputs) |inp| {
        const val = parseInt(inp) catch |err| {
            print("parseInt(\"{s}\") -> error: {s}\n", .{ inp, @errorName(err) });
            continue;
        };
        const result = safeDivide(val, 7) catch |err| {
            print("divide({d}, 7) -> error: {s}\n", .{ val, @errorName(err) });
            continue;
        };
        print("parseInt(\"{s}\") / 7 = {d}\n", .{ inp, result });
    }
}
