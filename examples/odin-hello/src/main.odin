// Odin example: procedures, parametric polymorphism, and defer.
package main

import "core:fmt"
import "core:math"
import "core:strings"

// Parametric polymorphism — resolved at compile time.
sum :: proc(values: []$T) -> T where intrinsics.type_is_numeric(T) {
    total: T
    for v in values { total += v }
    return total
}

Vec2 :: struct { x, y: f64 }

length :: proc(v: Vec2) -> f64 {
    return math.sqrt(v.x*v.x + v.y*v.y)
}

normalise :: proc(v: Vec2) -> Vec2 {
    l := length(v)
    if l == 0 { return v }
    return Vec2{v.x / l, v.y / l}
}

// defer runs at end of scope — handy for cleanup without RAII.
count_words :: proc(s: string) -> int {
    fields := strings.fields(s)
    defer delete(fields)  // free the slice when this proc returns
    return len(fields)
}

main :: proc() {
    fmt.println("Odin example")
    fmt.println("============")

    // Parametric sum
    ints   := []int{1, 2, 3, 4, 5}
    floats := []f64{0.1, 0.2, 0.3, 0.4}
    fmt.printf("sum(ints)   = %d\n",   sum(ints))
    fmt.printf("sum(floats) = %.1f\n", sum(floats))

    // Vec2 operations
    v := Vec2{3, 4}
    n := normalise(v)
    fmt.printf("\n|(%g, %g)| = %g\n", v.x, v.y, length(v))
    fmt.printf("normalise  = (%.4f, %.4f)\n", n.x, n.y)

    // defer
    sentence := "the quick brown fox jumps over the lazy dog"
    fmt.printf("\n\"%s\"\nwords: %d\n", sentence, count_words(sentence))
}
