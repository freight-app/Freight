const std = @import("std");
const print = std.debug.print;

// ── x86-64 assembly bindings (from math.asm) ─────────────────────────────────

extern fn asm_gcd(a: i64, b: i64) i64;
extern fn asm_popcount(n: u64) u64;
extern fn asm_bswap64(n: u64) u64;
extern fn asm_next_pow2(n: u64) u64;

pub fn main() void {
    // GCD via Euclidean division (IDIV)
    print("─── GCD (Euclidean via IDIV) ───\n", .{});
    const pairs = [_][2]i64{ .{48, 18}, .{100, 75}, .{1071, 462}, .{13, 7} };
    for (pairs) |p| {
        print("gcd({d:4}, {d:4}) = {d}\n", .{ p[0], p[1], asm_gcd(p[0], p[1]) });
    }

    // Population count (POPCNT)
    print("\n─── Population count (POPCNT) ───\n", .{});
    const words = [_]u64{ 0, 1, 0xFF, 0xDEAD_BEEF, 0xFFFF_FFFF_FFFF_FFFF };
    for (words) |w| {
        print("popcount(0x{x:016}) = {d}\n", .{ w, asm_popcount(w) });
    }

    // Byte-swap (BSWAP) — network byte-order conversion
    print("\n─── Byte swap (BSWAP) ───\n", .{});
    const val: u64 = 0x0102_0304_0506_0708;
    const swapped = asm_bswap64(val);
    print("0x{x:016} → 0x{x:016}\n", .{ val, swapped });
    print("round-trip: 0x{x:016}\n", .{asm_bswap64(swapped)});

    // Next power of two (BSR + SHL)
    print("\n─── Next power of two (BSR) ───\n", .{});
    const inputs = [_]u64{ 0, 1, 2, 3, 5, 8, 9, 100, 1023, 1024, 1025 };
    for (inputs) |n| {
        print("next_pow2({d:5}) = {d}\n", .{ n, asm_next_pow2(n) });
    }
}
