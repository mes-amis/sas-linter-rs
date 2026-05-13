proc format;
   value flagx 0 = 'OFF' 1 = 'ON';
run;

data foo;
   raw_score = 0;
   if raw_score >= 5 then total_score = 1;
   else total_score = 0;
   format total_score flagx.;
   attrib totalscore format=flagx.;
run;
