// @expected-stdout-begin
// const value: 10
// caught: Assignment to constant variable
// @expected-stdout-end
const x = 10;
console.log('const value: ' + x);
try {
  x = 20;
} catch (e) {
  console.log('caught: Assignment to constant variable');
}
