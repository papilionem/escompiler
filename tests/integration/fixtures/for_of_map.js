// @expected-stdout-begin
// a:1
// b:2
// c:3
// @expected-stdout-end
var m = new Map();
m.set("a", 1);
m.set("b", 2);
m.set("c", 3);
for (var [key, val] of m) {
    console.log(key + ":" + val);
}
