with Ada.Text_IO;
with Ada.Float_Text_IO;
with Ada.Numerics.Elementary_Functions;

procedure Main is

   use Ada.Text_IO;
   use Ada.Float_Text_IO;
   use Ada.Numerics.Elementary_Functions;

   -- ── Record type ───────────────────────────────────────────────────
   type Vec2 is record
      X, Y : Float;
   end record;

   -- ── Named array type ─────────────────────────────────────────────
   type Float_Arr is array (Integer range <>) of Float;

   -- ── Subtype with range constraint ────────────────────────────────
   subtype Score_Range is Integer range 0 .. 100;

   -- ── Functions on Vec2 ────────────────────────────────────────────
   function Add (A, B : Vec2) return Vec2 is
   begin
      return (X => A.X + B.X, Y => A.Y + B.Y);
   end Add;

   function Dot (A, B : Vec2) return Float is
   begin
      return A.X * B.X + A.Y * B.Y;
   end Dot;

   function Vec_Len (V : Vec2) return Float is
   begin
      return Sqrt (V.X ** 2 + V.Y ** 2);
   end Vec_Len;

   -- ── Insertion sort ───────────────────────────────────────────────
   procedure Isort (A : in out Float_Arr) is
      Key : Float;
      J   : Integer;
   begin
      for I in A'First + 1 .. A'Last loop
         Key := A (I);
         J   := I - 1;
         while J >= A'First and then Key < A (J) loop
            A (J + 1) := A (J);
            J         := J - 1;
         end loop;
         A (J + 1) := Key;
      end loop;
   end Isort;

   -- ── Declarations ─────────────────────────────────────────────────
   A, B, C : Vec2;
   Data    : constant Float_Arr (1 .. 8) :=
      (4.0, 8.0, 15.0, 16.0, 23.0, 42.0, 3.0, 7.0);
   Sorted  : Float_Arr (1 .. 8);
   Total   : Float := 0.0;

begin

   -- ── Vec2 arithmetic ──────────────────────────────────────────────
   A := (X => 3.0, Y => 4.0);
   B := (X => 1.0, Y => 2.0);
   C := Add (A, B);
   Put ("a + b = (");
   Put (C.X, Fore => 1, Aft => 1, Exp => 0);
   Put (", ");
   Put (C.Y, Fore => 1, Aft => 1, Exp => 0);
   Put_Line (")");
   Put ("dot    = ");
   Put (Dot (A, B), Fore => 1, Aft => 4, Exp => 0);
   New_Line;
   Put ("|a|    = ");
   Put (Vec_Len (A), Fore => 1, Aft => 4, Exp => 0);
   New_Line;

   -- ── Array: mean ──────────────────────────────────────────────────
   for V of Data loop
      Total := Total + V;
   end loop;
   Put ("mean   = ");
   Put (Total / Float (Data'Length), Fore => 1, Aft => 4, Exp => 0);
   New_Line;

   -- ── Insertion sort ───────────────────────────────────────────────
   Sorted := Data;
   Isort (Sorted);
   Put_Line ("sorted =");
   for V of Sorted loop
      Put (V, Fore => 4, Aft => 0, Exp => 0);
   end loop;
   New_Line;

   -- ── Exception handling: subtype constraint ───────────────────────
   declare
      Score : Score_Range;
   begin
      Score := 85;
      Put_Line ("score =" & Integer'Image (Score) & "%");
      Score := 101;
   exception
      when Constraint_Error =>
         Put_Line ("caught Constraint_Error: 101 out of range 0..100");
   end;

end Main;
