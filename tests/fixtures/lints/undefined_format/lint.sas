proc format;
   value sixscore 0 = 'Independent'
                  6 = 'Total dependence';
run;

data foo;
   sSCORE = 0;
   attrib sSCORE format = sscore.;
run;
