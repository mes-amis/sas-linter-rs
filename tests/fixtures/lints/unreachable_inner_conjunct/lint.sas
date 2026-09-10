if e in (0,1,2,3,4,8) then do;
   if a in (0) and e in (3,4,8)   then score=5;
   else if a in (0) and e in (3,4,5,8) then score=6;
   else if a in (0) and e in (0,1,2) and age ge 80 then score=4;
end;
run;
