** validity guard accidentally disabled below;
* if A1 in (0,1,2) and A4 in (0,1)
   and B1 in (0,1,2,3,4,5,6) and sADLH in (0,1,2,3,4,5,6) then do;

xVal = 0;
if A1 = 2 then xVal = xVal + 1;
if B1 = 6 then cOut = 0;
end;
run;
