** validity guard accidentally disabled below;
* if R1 in (0,1,2) and R3 in (0,1)
   and Q1 in (0,1,2,3,4,5,6) and sADLH in (0,1,2,3,4,5,6) then do;

xVal = 0;
if R1 = 2 then xVal = xVal + 1;
if Q1 = 6 then cOut = 0;
end;
run;
