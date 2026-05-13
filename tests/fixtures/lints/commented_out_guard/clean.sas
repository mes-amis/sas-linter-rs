** Plain narrative comment that mentions an if statement, no do block;
** TODO: revisit the A12a calculation when the new schema lands;
* xRetired = 0;
* xUnused = 1;

if A1 in (0,1,2) and B1 in (0,1,2,3,4,5,6) then do;
   xVal = 0;
   if A1 = 2 then xVal = xVal + 1;
   if B1 = 6 then cOut = 0;
end;
run;
