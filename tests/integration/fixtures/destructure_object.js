// Test object destructuring assignment
// @expected-stdout: hello 42

let x, y;
let obj = { x: "hello", y: 42 };
({ x, y } = obj);
console.log(x, y);
