proc format;
   value fmtname 0 = 'No'
                 1 = 'Yes';
   invalue $abc 'a' = 1;
   picture pct low-high = '009%';
run;

proc sort data=foo out=bar;
   by id;
run;
