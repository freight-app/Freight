// Swift example: generics, protocols, and value types.

protocol Shape {
    var area: Double { get }
    var perimeter: Double { get }
    var name: String { get }
}

struct Circle: Shape {
    let radius: Double
    var name: String { "Circle(r=\(radius))" }
    var area: Double { Double.pi * radius * radius }
    var perimeter: Double { 2 * Double.pi * radius }
}

struct Rectangle: Shape {
    let width: Double
    let height: Double
    var name: String { "Rectangle(\(width)×\(height))" }
    var area: Double { width * height }
    var perimeter: Double { 2 * (width + height) }
}

func printShape<S: Shape>(_ s: S) {
    print(String(format: "  %-24s  area=%8.3f  perimeter=%8.3f",
        s.name, s.area, s.perimeter))
}

let shapes: [any Shape] = [
    Circle(radius: 5),
    Circle(radius: 1),
    Rectangle(width: 4, height: 3),
    Rectangle(width: 10, height: 2),
]

print("Shapes:")
for shape in shapes {
    printShape(shape)
}

let totalArea = shapes.reduce(0) { $0 + $1.area }
print(String(format: "\nTotal area: %.3f", totalArea))
