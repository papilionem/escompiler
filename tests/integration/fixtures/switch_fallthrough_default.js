// @expected-stdout-begin
// 6
// 4
// 56
// 48
// 64
// 32
// 32
// 32
// 32
// 32
// @expected-stdout-end
function SwitchTest(value){
  var result = 0;

  switch(value) {
    case 0:
      result += 2;
    case 1:
      result += 4;
      break;
    case 2:
      result += 8;
    case 3:
      result += 16;
    default:
      result += 32;
      break;
    case 4:
      result += 64;
  }

  return result;
}

console.log(SwitchTest(0));
console.log(SwitchTest(1));
console.log(SwitchTest(2));
console.log(SwitchTest(3));
console.log(SwitchTest(4));
console.log(SwitchTest(true));
console.log(SwitchTest(false));
console.log(SwitchTest(null));
console.log(SwitchTest(undefined));
console.log(SwitchTest('0'));
