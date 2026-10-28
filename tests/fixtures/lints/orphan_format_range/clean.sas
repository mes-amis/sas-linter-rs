proc format;
   value levels 0 = 'Low'
                1 = 'Medium'
                2 = 'High';
   value $yn 'Y' = 'Yes' other = 'No';
run;

data foo;
   x = 1;
   if x = 1 then y = 2;
run;
