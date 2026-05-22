program main
    use iso_fortran_env, only: real64
    implicit none

    ! ── Derived type ──────────────────────────────────────────────────────────
    type :: Vec3
        real(real64) :: x, y, z
    end type Vec3

    ! ── Elemental functions ───────────────────────────────────────────────────
    interface
        elemental function clamp(v, lo, hi) result(r)
            import real64
            real(real64), intent(in) :: v, lo, hi
            real(real64) :: r
        end function
    end interface

    type(Vec3) :: a, b, c
    real(real64) :: data(10), sorted(10)
    integer :: i

    ! ── Vec3 arithmetic ───────────────────────────────────────────────────────
    a = Vec3(1.0d0, 2.0d0, 3.0d0)
    b = Vec3(4.0d0, 5.0d0, 6.0d0)
    c = Vec3(a%x+b%x, a%y+b%y, a%z+b%z)
    print '(a,3f6.1,a)', "a + b = (", c%x, c%y, c%z, " )"

    print '(a,f8.4)', "dot(a,b) = ", a%x*b%x + a%y*b%y + a%z*b%z

    ! ── Array intrinsics ─────────────────────────────────────────────────────
    data = [4.d0, 8.d0, 15.d0, 16.d0, 23.d0, 42.d0, 3.d0, 7.d0, 1.d0, 9.d0]
    print '(a,f8.4)', "mean   = ", sum(data) / size(data)
    print '(a,f8.4)', "maxval = ", maxval(data)
    print '(a,f8.4)', "minval = ", minval(data)

    ! ── Insertion sort using do-loops ─────────────────────────────────────────
    sorted = data
    call isort(sorted, size(sorted))
    print '(a)', "sorted ="
    print '(10f6.0)', sorted

contains

    subroutine isort(arr, n)
        integer, intent(in) :: n
        real(real64), intent(inout) :: arr(n)
        integer :: i, j
        real(real64) :: key
        do i = 2, n
            key = arr(i)
            j = i - 1
            do while (j >= 1 .and. arr(j) > key)
                arr(j+1) = arr(j)
                j = j - 1
            end do
            arr(j+1) = key
        end do
    end subroutine

end program main

elemental function clamp(v, lo, hi) result(r)
    use iso_fortran_env, only: real64
    real(real64), intent(in) :: v, lo, hi
    real(real64) :: r
    r = min(max(v, lo), hi)
end function
