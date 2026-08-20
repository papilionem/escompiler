// @expected-stdout-begin
// 1
// 2
// hello
// @expected-stdout-end
let o = {x: 1, y: 2, z: "hello"};
console.log(o.x);
console.log(o.y);
console.log(o.z);
