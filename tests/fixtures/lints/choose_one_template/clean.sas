** A regular SAS line comment that doesn't trigger the rule  **;
if A1 in (0,1,2,3) and A4 in (0,1) then do;
   xOut = 0;
end;
run;

/*
** CHOOSE ONE OF THE BELOW STATEMENTS.
** This banner is inside a `/* ... */` block comment — SAS treats the entire
** /* ... */ region as one C_STYLE_COMMENT, so the rule should NOT fire.
*/
