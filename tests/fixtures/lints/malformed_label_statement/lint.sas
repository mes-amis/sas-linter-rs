data foo;
   set bar;
   xrisk = 0;
   if gTwo in (1,2) then xrisk = xrisk + 1;
**  LABEL TRIGGER  **;
label myLabel 'Delirium Screener';
end;
run;
