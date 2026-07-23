data score_calc;
   set input;
   total = 0;
   if a1 in (1,2,3) then total = total + 1; * first item note:
   if a2 in (4,5)   then total = total + 1; * second item note;
   if a3 = 1        then total = total + 1; * third item note:
   if a4 in (2,3)   then total = total + 1; * fourth item note;
   result = total / 4;
run;
