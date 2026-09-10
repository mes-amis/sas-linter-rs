if e in (0,1,2,3,4,8) then do;
   if a in (0) and e in (3,4,8)   then score=5;
   else if a in (0) and e in (0,1,2) and age ge 80 then score=4;
   else if a in (0) and e in (0,1,2) and age lt 80 then score=3;
   else if e in (9) or a = 1 then score=2;
end;
run;
