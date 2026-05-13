data foo;
   set bar;
   xrisk = 0;
   if iJ1g in (1,2) then xrisk = xrisk + 1;
**  LABEL TRIGGER  **;
label aHSDELIRIUM = 'Delirium Screener';
end;
run;
