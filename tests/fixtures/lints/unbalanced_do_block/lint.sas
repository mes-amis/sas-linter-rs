**  DATA-STEP FRAGMENT: caller supplies data/set and run;                    **;
if raw_score = 0 then risk_band = 0;
else do;
   if comorbidity = 1 then risk_band = 1;
   else do;
      risk_band = 2;
