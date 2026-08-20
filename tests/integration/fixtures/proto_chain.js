// @expected-stdout-begin
// 1
// 2
// @expected-stdout-end
let parent = { a: 1 };
let child = Object.create(parent);
child.b = 2;
console.log(child.a);
console.log(child.b);
