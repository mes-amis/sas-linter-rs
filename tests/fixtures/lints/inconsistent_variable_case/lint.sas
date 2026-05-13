proc format;
   value myFlag 0 = 'OFF' 1 = 'ON';
run;

data x;
   format myFlag myFlag.;
   if myFlag = 0 then MyFlag = 0;
   else if myFlag = 1 then myFlag = 1;
   else myFlag = .;
run;
