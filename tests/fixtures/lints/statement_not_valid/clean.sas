** Program title **;
options nocenter;
title1 'Report';
libname lib 'path';
%let cutoff = 5;
data foo;
   set bar;
   retain total 0;
   array a{3} a1-a3;
   a{1} = 0;
   a(2) = 0;
   'my var'n = 1;
   total + 1;
   h.add();
   %mymacro(x);
   call missing(y);
   select (x);
      when (1) y = 1;
      otherwise y = 2;
   end;
   if x then do;
      leave;
   end;
   else return;
skip: y = 3;
   putlog 'done';
   output;
   datalines;
1 2 3
;
run;

proc sql;
   create table t as select * from foo;
quit;

proc print data=foo;
   var x;
run;
