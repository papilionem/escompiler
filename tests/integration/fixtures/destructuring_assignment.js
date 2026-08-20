// @expected-stdout: 10 20
let x, y;
({ x, y } = { x: 10, y: 20 });
console.log(x, y);
