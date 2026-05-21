import std.stdio   : writeln, writefln;
import std.format  : format;
import std.math    : sqrt, PI;
import std.range   : iota;
import std.algorithm.iteration : map, filter;
import std.algorithm.searching : minElement, maxElement;
import std.array   : array;

// ── Vec2 — operator overloading and properties ────────────────────────────────

struct Vec2 {
    double x, y;

    Vec2 opAdd(Vec2 rhs) const { return Vec2(x + rhs.x, y + rhs.y); }
    Vec2 opSub(Vec2 rhs) const { return Vec2(x - rhs.x, y - rhs.y); }
    Vec2 opMul(double s) const { return Vec2(x * s, y * s); }

    double length() const @property { return sqrt(x*x + y*y); }
    Vec2   normalized() const @property { return this * (1.0 / length); }

    double dot(Vec2 rhs) const { return x * rhs.x + y * rhs.y; }

    string toString() const { return format("(%.3f, %.3f)", x, y); }
}

// ── Fibonacci via lazy range ───────────────────────────────────────────────────

struct Fibonacci {
    ulong a = 0, b = 1;

    bool   empty()  const @property { return false; }
    ulong  front()  const @property { return a; }
    void   popFront() { auto tmp = a + b; a = b; b = tmp; }
}

// ── C interop — call libc qsort directly ──────────────────────────────────────

extern (C) void qsort(void* base, size_t nmemb, size_t size,
                      int function(const void*, const void*) nothrow @nogc compar) nothrow @nogc;

extern (C) int cmp_double(const void* a, const void* b) nothrow @nogc {
    double da = *cast(const double*)a;
    double db = *cast(const double*)b;
    return (da > db) - (da < db);
}

void main() {
    // ── Range pipeline ─────────────────────────────────────────────────────
    writeln("─── Range pipeline ───");
    auto evens = iota(1, 21).filter!(n => n % 2 == 0).array;
    auto squares = evens.map!(n => n * n).array;

    writefln("Even numbers [1,20]:        %s", evens);
    writefln("Their squares:              %s", squares);
    writefln("Min square: %d  Max square: %d", squares.minElement, squares.maxElement);

    // ── Vec2 arithmetic ────────────────────────────────────────────────────
    writeln("\n─── Vec2 arithmetic ───");
    Vec2 a = Vec2(3.0, 4.0);
    Vec2 b = Vec2(1.0, -2.0);
    writefln("a            = %s  length = %.3f", a, a.length);
    writefln("b            = %s  length = %.3f", b, b.length);
    writefln("a + b        = %s", a + b);
    writefln("a.normalized = %s  (length ≈ 1: %.6f)", a.normalized, a.normalized.length);
    writefln("a · b        = %.3f", a.dot(b));

    // ── Fibonacci via lazy range ────────────────────────────────────────────
    writeln("\n─── First 12 Fibonacci numbers ───");
    Fibonacci fib;
    ulong[] nums;
    foreach (_; 0 .. 12) {
        nums ~= fib.front;
        fib.popFront();
    }
    writefln("%s", nums);

    // ── C interop: qsort ────────────────────────────────────────────────────
    writeln("\n─── C interop: qsort ───");
    double[] data = [3.14, -1.0, 2.72, 0.0, 1.41, -0.5];
    writefln("before: %s", data);
    qsort(data.ptr, data.length, double.sizeof, &cmp_double);
    writefln("after:  %s", data);
}
