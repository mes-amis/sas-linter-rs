**  DATA-STEP FRAGMENT: caller supplies data/set and run;                    **;
if raw_score = 0 then risk_band = 0;
else do;
   if comorbidity = 1 then risk_band = 1;
   else do;
      risk_band = 2;
   end;
end;

do i = 1 to 10;
   do while (running < 100);
      running = running + i;
   end;
   do until (done);
      done = 1;
   end;
end;

do over item_arr;
   item_arr = item_arr + 1;
end;

select (raw_score);
   when (0) risk_band = 0;
   when (1) do;
      risk_band = 1;
   end;
   otherwise do;
      risk_band = 9;
   end;
end;

* an end; inside a comment does not count;
weekend_flag = 1;
xEndDate     = today();
note_text    = 'end;';
