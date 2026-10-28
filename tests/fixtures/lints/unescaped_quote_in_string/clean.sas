data foo;
   score = 1;
   when = '01jan2020'd;
   label score = "Children's Algorithm score 0-6 range"
         when  = 'Children''s visit date';
   if score = 1 and name='a'then flag = 1;
   'my var'n = 2;
run;
