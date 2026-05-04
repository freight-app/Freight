// Zig example: comptime, tagged unions, and error sets.
const std = @import("std");

// Tagged union — Zig's safe equivalent of C's untagged union + enum.
const Value = union(enum) {
    int: i64,
    float: f64,
    boolean: bool,

    pub fn format(self: Value, comptime _: []const u8, _: std.fmt.FormatOptions, writer: anytype) !void {
        switch (self) {
            .int     => |v| try writer.print("{d}", .{v}),
            .float   => |v| try writer.print("{d:.3}", .{v}),
            .boolean => |v| try writer.print("{}", .{v}),
        }
    }
};

// Comptime generic: type is resolved at compile time, zero runtime overhead.
fn sumSlice(comptime T: type, values: []const T) T {
    var total: T = 0;
    for (values) |v| total += v;
    return total;
}

// Zig error handling: explicit error sets, no hidden exceptions.
const ParseError = error{ Empty, InvalidChar };

fn parsePositive(s: []const u8) ParseError!u64 {
    if (s.len == 0) return error.Empty;
    var result: u64 = 0;
    for (s) |ch| {
        if (ch < '0' or ch > '9') return error.InvalidChar;
        result = result * 10 + (ch - '0');
    }
    return result;
}

pub fn main() !void {
    const stdout = std.io.getStdOut().writer();

    // Tagged union
    const values = [_]Value{
        .{ .int = 42 },
        .{ .float = 3.14159 },
        .{ .boolean = true },
    };
    try stdout.print("Values:\n", .{});
    for (values) |v| try stdout.print("  {}\n", .{v});

    // Comptime generics
    const ints = [_]i64{ 1, 2, 3, 4, 5 };
    try stdout.print("\nsum(1..5) = {d}\n", .{sumSlice(i64, &ints)});

    const floats = [_]f64{ 0.1, 0.2, 0.3 };
    try stdout.print("sum(0.1+0.2+0.3) = {d:.1}\n", .{sumSlice(f64, &floats)});

    // Error handling
    const inputs = [_][]const u8{ "123", "", "4x5" };
    try stdout.print("\nparsePositive:\n", .{});
    for (inputs) |s| {
        const result = parsePositive(s);
        if (result) |n| {
            try stdout.print("  \"{s}\" => {d}\n", .{ s, n });
        } else |err| {
            try stdout.print("  \"{s}\" => error.{s}\n", .{ s, @errorName(err) });
        }
    }
}
