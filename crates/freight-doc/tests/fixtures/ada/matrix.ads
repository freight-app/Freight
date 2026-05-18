--! @brief 2×2 matrix arithmetic package.
--! Provides basic dense-matrix operations on double-precision values.
package Matrix is

   --! Element type alias.
   subtype Element is Float;

   --! 2×2 matrix stored in row-major order.
   type Mat2 is array (1 .. 2, 1 .. 2) of Element;

   --! Multiply two 2×2 matrices.
   --! @param A Left factor.
   --! @param B Right factor.
   --! @return  A * B
   function Mul (A, B : Mat2) return Mat2;

   --! Add two matrices element-wise.
   --! @param A First matrix.
   --! @param B Second matrix.
   --! @return  A + B
   function Add (A, B : Mat2) return Mat2;

   --! Compute determinant of a 2×2 matrix.
   --! @param A Input matrix.
   --! @return  det(A) = A(1,1)*A(2,2) - A(1,2)*A(2,1)
   function Det (A : Mat2) return Element;

   --! Transpose a 2×2 matrix.
   --! @param A Matrix to transpose.
   --! @return  Aᵀ
   function Transpose (A : Mat2) return Mat2;

end Matrix;
