// @expected-stdout: Value is 42!
function tag(strings, ...values) {
  return strings[0] + values[0] + strings[1];
}
let x = 42;
console.log(tag`Value is ${x}!`);
