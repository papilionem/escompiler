// @expected-stdout-begin
// PASS
// @expected-stdout-end
function greet(greeting) {
    return greeting + " " + this.name;
}
var obj = {name: "World"};
var result = greet.call(obj, "Hello");
if (result !== "Hello World") throw "FAIL: call got " + result;
var result2 = greet.apply(obj, ["Hi"]);
if (result2 !== "Hi World") throw "FAIL: apply got " + result2;
console.log("PASS");
