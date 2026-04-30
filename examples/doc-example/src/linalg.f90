!> @file linalg.f90
!> @brief Basic linear algebra routines — Fortran with FORD-style comments.

module linalg
    implicit none

    !> Double-precision alias for clarity.
    integer, parameter :: dp = kind(1.0d0)

contains

    !> Compute the dot product of two real vectors.
    !! Iterates over the common length; shorter vector is treated as zero-padded.
    !! @param u  First vector.
    !! @param v  Second vector.
    !! @return   u · v
    pure function dot(u, v) result(res)
        real(dp), intent(in) :: u(:), v(:)
        real(dp) :: res
        integer :: i, n
        n = min(size(u), size(v))
        res = 0.0_dp
        do i = 1, n
            res = res + u(i) * v(i)
        end do
    end function dot

    !> Scale a vector by a scalar factor in-place.
    !! @param v      Vector to scale (modified).
    !! @param alpha  Scale factor.
    subroutine scale(v, alpha)
        real(dp), intent(inout) :: v(:)
        real(dp), intent(in)    :: alpha
        v = v * alpha
    end subroutine scale

    !> Euclidean norm of a vector.
    !! @param v  Input vector.
    !! @return   ||v||₂
    pure function norm2(v) result(res)
        real(dp), intent(in) :: v(:)
        real(dp) :: res
        res = sqrt(dot(v, v))
    end function norm2

    !> Solve a 2×2 linear system Ax = b via Cramer's rule.
    !!
    !! Returns .false. if A is singular (det ≈ 0).
    !!
    !! @param A    2×2 coefficient matrix.
    !! @param b    Right-hand side vector (length 2).
    !! @param x    Solution vector (output, length 2).
    !! @return     .true. on success, .false. if singular.
    logical function solve2(A, b, x)
        real(dp), intent(in)  :: A(2,2), b(2)
        real(dp), intent(out) :: x(2)
        real(dp) :: det
        det = A(1,1)*A(2,2) - A(1,2)*A(2,1)
        if (abs(det) < 1.0e-14_dp) then
            solve2 = .false.
            x = 0.0_dp
            return
        end if
        x(1) = (b(1)*A(2,2) - b(2)*A(1,2)) / det
        x(2) = (A(1,1)*b(2) - A(2,1)*b(1)) / det
        solve2 = .true.
    end function solve2

end module linalg
