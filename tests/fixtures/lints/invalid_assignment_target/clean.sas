data outcomes;
  set responses;

  file log linesize = 80;

  score = -1.5 + 0.2 * age;

  Predicted_risk = exp(score) / (1 + exp(score));

  label Predicted_risk = "Predicted risk";

run;

proc reg data=outcomes;
  model Predicted_risk = score;
run;
