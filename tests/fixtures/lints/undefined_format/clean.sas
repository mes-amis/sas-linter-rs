proc format;
   value sixscore 0 = 'Independent'
                  6 = 'Total dependence';
   value $yn 'Y' = 'Yes' 'N' = 'No';
run;

data foo;
   sSCORE = 0;
   flag = 'Y';
   attrib sSCORE format = SixScore. informat = best12.;
   format flag $yn. when date9. amount dollar12.2 code $char10. width 8.;
run;
