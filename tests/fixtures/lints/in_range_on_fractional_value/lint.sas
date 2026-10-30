data scored;
   set measures;
   bmi = weight / (height * height);
   if age in (0:130) and bmi in (5:70) then do;
      if bmi < 18 then score = 7;
      else score = 3;
   end;
run;
