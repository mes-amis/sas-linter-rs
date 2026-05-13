data foo;
   keep score;
   retain score 0;
   score = 5;
   format score best.;
run;
