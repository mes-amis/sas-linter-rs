****************************************************************************************;
**  PROGRAM:          OK.TXT                                                         **;
****************************************************************************************;

data one; set have;
*** Sum the number of contributing components.
Maximum is 4, but is 2 in the short form because two components are dropped;

total=a + b + c + d;
run;
