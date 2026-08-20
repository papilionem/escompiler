// @expected-stdout-begin
// 42
// hello
// true
// @expected-stdout-end
let original = {x: 42, name: "hello", active: true};
let json = JSON.stringify(original);
let parsed = JSON.parse(json);
console.log(parsed.x);
console.log(parsed.name);
console.log(parsed.active);
