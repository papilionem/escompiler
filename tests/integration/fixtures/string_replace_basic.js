// @expected-stdout-begin
// hello earth
// herlo
// herro
// @expected-stdout-end
var s = "hello world".replace("world", "earth");
console.log(s);
var t = "hello".replace("l", "r");
console.log(t);
var u = "hello".replaceAll("l", "r");
console.log(u);
