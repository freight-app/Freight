/**
 * @brief 2-D and 3-D geometry primitives.
 */
#pragma once

namespace geometry {

/**
 * @brief 2-D point with floating-point coordinates.
 * @tparam T Coordinate type (float or double).
 */
template<typename T>
struct Point {
    T x; ///< X coordinate.
    T y; ///< Y coordinate.

    /**
     * @brief Distance from this point to the origin.
     * @return Euclidean norm.
     */
    double length() const;

    /**
     * @brief Translate by an offset.
     * @param dx X offset.
     * @param dy Y offset.
     */
    void translate(T dx, T dy);
};

/**
 * @brief Axis-aligned bounding box.
 * @tparam T Coordinate type.
 */
template<typename T>
class AABB {
public:
    /**
     * @brief Construct from min/max corners.
     * @param lo Minimum corner (lower-left).
     * @param hi Maximum corner (upper-right).
     */
    explicit AABB(Point<T> lo, Point<T> hi);

    /**
     * @brief Test whether a point lies inside or on the boundary.
     * @param p Point to test.
     * @return true if p is contained.
     */
    bool contains(Point<T> p) const;

    /**
     * @brief Expand the box to include a point.
     * @param p Point to include.
     */
    void expand(Point<T> p);
};

/**
 * @brief Signed area of a triangle defined by three vertices.
 *
 * Positive when vertices are counter-clockwise.
 *
 * @param a First vertex.
 * @param b Second vertex.
 * @param c Third vertex.
 * @return  Signed area.
 */
double triangle_area(Point<double> a, Point<double> b, Point<double> c);

/**
 * @brief Test whether two AABBs overlap.
 * @param x First box.
 * @param y Second box.
 * @return  true if they share any interior or boundary point.
 */
bool aabb_overlaps(AABB<double> x, AABB<double> y);

} // namespace geometry
