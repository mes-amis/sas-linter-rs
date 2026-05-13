proc format;
   value flagx 0 = 'A' 1 = 'B';
run;
data foo;
   real_var = 1;
   attrib v format=flagx.;
run;
