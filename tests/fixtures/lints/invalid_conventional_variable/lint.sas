data work.scored;
  set raw.entries;
  if ABC_1 = 1 then score = 1;
  if ABC_1000 = 1 then score = 2;        /* typo: catalog has ABC_100 */
  if QRS_99 = 1 then score = 3;          /* not in catalog, no close match */
  if ABC_101 = 1 then comment = "typoed ABC_100";
run;
