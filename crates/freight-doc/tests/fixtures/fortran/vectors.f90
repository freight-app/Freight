!> @file vectors.f90
!> @brief 3-D vector operations with FORD-style doc comments.

module vectors
    implicit none

    !> Working precision (double).
    integer, parameter :: wp = kind(1.0d0)

    !> Maximum supported vector dimension.
    integer, parameter :: max_dim = 3

    !> Small threshold used by normalise to avoid division by zero.
    real(wp), parameter :: norm_tol = 1.0e-15_wp

contains

    !> Dot product of two 3-vectors.
    !! @param u First vector (length 3).
    !! @param v Second vector (length 3).
    !! @return  u·v
    pure function dot3(u, v) result(res)
        real(wp), intent(in) :: u(3), v(3)
        real(wp) :: res
        res = u(1)*v(1) + u(2)*v(2) + u(3)*v(3)
    end function dot3

    !> Cross product of two 3-vectors.
    !! @param u First vector.
    !! @param v Second vector.
    !! @return  u×v
    pure function cross3(u, v) result(res)
        real(wp), intent(in) :: u(3), v(3)
        real(wp) :: res(3)
        res(1) = u(2)*v(3) - u(3)*v(2)
        res(2) = u(3)*v(1) - u(1)*v(3)
        res(3) = u(1)*v(2) - u(2)*v(1)
    end function cross3

    !> Euclidean norm of a 3-vector.
    !! @param v Input vector.
    !! @return  ||v||₂
    pure function norm3(v) result(res)
        real(wp), intent(in) :: v(3)
        real(wp) :: res
        res = sqrt(dot3(v, v))
    end function norm3

    !> Normalise a 3-vector in-place.
    !! Does nothing if the vector is shorter than `norm_tol`.
    !! @param v Vector to normalise (modified in place).
    subroutine normalise(v)
        real(wp), intent(inout) :: v(3)
        real(wp) :: len
        len = norm3(v)
        if (len > norm_tol) v = v / len
    end subroutine normalise

end module vectors
