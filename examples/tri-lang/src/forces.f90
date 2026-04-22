! N-body pairwise gravitational forces.
! All arrays passed as C pointers via iso_c_binding.
module forces_mod
  use iso_c_binding, only: c_int, c_double
  implicit none
contains

  subroutine gravity(n, x, y, mass, fx, fy) bind(C, name="gravity")
    integer(c_int), value, intent(in) :: n
    real(c_double), intent(in)        :: x(n), y(n), mass(n)
    real(c_double), intent(out)       :: fx(n), fy(n)

    integer(c_int) :: i, j
    real(c_double) :: dx, dy, r2, inv_r3

    fx = 0.0d0
    fy = 0.0d0

    do i = 1, n
      do j = 1, n
        if (i == j) cycle
        dx     = x(j) - x(i)
        dy     = y(j) - y(i)
        r2     = dx*dx + dy*dy + 1.0d-10  ! softening avoids singularity
        inv_r3 = 1.0d0 / (r2 * sqrt(r2))
        fx(i)  = fx(i) + mass(j) * dx * inv_r3
        fy(i)  = fy(i) + mass(j) * dy * inv_r3
      end do
    end do
  end subroutine gravity

end module forces_mod
