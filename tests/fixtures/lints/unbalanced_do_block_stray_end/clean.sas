**  DATA-STEP FRAGMENT: caller supplies data/set and run;                    **;
if raw_score = 0 then risk_band = 0;
else do;
   risk_band = 1;
end;

set source_data end = eof;
if end_of_month then flush_flag = 1;
