data score_calc;
   set input;
   total = 0;
   if a1 in (1,2,3) then total = total + 1; * first item note;
   if a2 in (4,5)   then total = total + 1; * second item note;
   * a full-line note that legitimately wraps
     onto a second line and ends here;
   /* a c-style block
      spanning several lines */
   result = total / 4;
run;
