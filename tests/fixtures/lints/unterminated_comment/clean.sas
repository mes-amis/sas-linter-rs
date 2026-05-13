data foo;
   set bar;
   x = 1;
   ** SOME COMMENT **;
   y = x + 1;
   ** ANOTHER NOTE **;
   z = y * 2;
   * a long prose comment that legitimately wraps
     across multiple lines and ends here;
   ** properly closed multi-line
      header-shaped comment **;
run;
