data outcomes;
  set responses;

  score = -1.5 + 0.2 * age;

  Predicted risk = exp(score) / (1 + exp(score));

run;
