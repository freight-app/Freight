#include <cstdio>
#include <cmath>
import vec3;
import ray;

// Colour a pixel: background gradient or a sphere hit.
Vec3 colour(const Ray& r) {
    double t = hit_sphere({0, 0, -1}, 0.5, r);
    if (t > 0.0) {
        Vec3 n = (r.at(t) - Vec3{0, 0, -1}).normalise();
        return (n + Vec3{1, 1, 1}) * 0.5;
    }
    Vec3   unit = r.direction.normalise();
    double a    = 0.5 * (unit.y + 1.0);
    return Vec3{1, 1, 1} * (1.0 - a) + Vec3{0.5, 0.7, 1.0} * a;
}

int main() {
    constexpr int W = 40, H = 20;

    Vec3 origin{0, 0, 0};
    Vec3 horizontal{4, 0, 0};
    Vec3 vertical{0, 2, 0};
    Vec3 lower_left{-2, -1, -1};

    for (int j = H - 1; j >= 0; j--) {
        for (int i = 0; i < W; i++) {
            double u = static_cast<double>(i) / (W - 1);
            double v = static_cast<double>(j) / (H - 1);
            Ray r{ origin, (lower_left + horizontal*u + vertical*v).normalise() };
            Vec3 c = colour(r);

            double lum = 0.299*c.x + 0.587*c.y + 0.114*c.z;
            const char* shades = " .:-=+*#%@";
            std::putchar(shades[static_cast<int>(lum * 9.99)]);
        }
        std::putchar('\n');
    }
    return 0;
}
