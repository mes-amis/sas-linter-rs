data work.example;
set sashelp.class;
age_group = 0;
if age > 12 then do;
age_group = 2;
height = height * 1.1;
end;
else do;
age_group = 1;
end;
run;

proc sort data=work.example;
by name;
run;
