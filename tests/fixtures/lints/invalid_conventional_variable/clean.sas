data work.scored;
  set raw.entries;
  if ABC_1 = 1 then score = 1;
  if ABC_2 = 1 then score = 2;
  if ABC_100 = 1 then score = 3;
  if XYZ_42 = 1 then total = total + 1;
  if DEF_7 = 1 then comment = "documented";
  /* names that don't match the pattern are ignored */
  if myVar = 1 then score = score + 1;
  if v100 = 1 then score = score - 1;
run;
