data work.out;
x=a+b-c;
y=a*b/c;
z=a**2;
if age>12 and height<=65 then cOutB=1;
check=x in(0,1,2);
label check='valid',x='age';
x=x ;
run;
