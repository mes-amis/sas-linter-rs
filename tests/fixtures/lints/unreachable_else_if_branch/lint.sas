if a in (0) and b in (1,2,3,4) and c in (0) then do;
   if d in (1) and e in (3,4,8)              then score=5;
   else if d in (1) and e in (0,1,2) and age ge 80 then score=4;
   else if d in (1) and e in (0,1,2) and age lt 80 then score=3;
   else if d in (2) and e in (0,1,2)              then score=3;
   else if d in (2) and e in (3,4,8)              then score=4;
   else if d in (1) and e in (3,4,8)              then score=5;
end;
run;
