if STAGE_VAR in (0,1,2,3,4,5,6,7,8) then do;
   if STAGE_VAR in (0,1) then cOut = 0;
   if STAGE_VAR in (2,3,4) then cOut = 1;
   if STAGE_VAR in (5,6,7,8) then cOut = 2;
end;
run;
