if R1 = 1 and R2 = 2 then cOut = 1;
if R1 = 1 or R2 = 2 then cOut = 2;
if R1 = 1 then cOut = 3;
if R1 then cOut = 4;
if not R1 = 0 then cOut = 5;
if (R1 = 1 and R2 = 2) or gThree = 3 then cOut = 6;
if R1 in (1,2,3) then cOut = 7;
if abs(R1 - R2) > 5 then cOut = 8;
if R1 = 1 then do; cOut = 9; end;
if R1 = 2 then cOut = 10; else cOut = 11;
if R1; cOut = 12;
if R1 = 1
   and R2 = 2
   then cOut = 13;

** Regression cases for issue #1 — visual comments (** … **;) must not be
   analysed as code, even when prose contains keywords or embedded ';' **;
**  REVISION DATES:   01/01/00; 02/02/01 (fixed typo: ... or x = 1 then do;)  **;
**  ALGORITHM: Decision tree splits sample by group A, then by group B.       **;
**  Multi-line visual comment style — each line is its own **…**; statement   **;
**              by cognitive status (sCPS) and behavioral problems, then by other **;
**VARIABLE ASSIGNMENTS**; **PUT YOUR VARIABLES ON THE RIGHT-HAND SIDE HERE**;
