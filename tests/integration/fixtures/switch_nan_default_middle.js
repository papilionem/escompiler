// @expected-stdout-begin
// 6
// 4
// 56
// 48
// 32
// 192
// 32
// 768
// 1024
// @expected-stdout-end

// Switch with NaN (never matches), null, undefined, Infinity, and default in middle
function SwitchTest(value) {
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
        case null:
            result += 64;
        case NaN:
            result += 128;
            break;
        case Infinity:
            result += 256;
        case 5:
            result += 512;
            break;
        case undefined:
            result += 1024;
    }

    return result;
}

console.log(SwitchTest(0));
console.log(SwitchTest(1));
console.log(SwitchTest(2));
console.log(SwitchTest(3));
console.log(SwitchTest(true));
console.log(SwitchTest(null));
console.log(SwitchTest(NaN));
console.log(SwitchTest(Infinity));
console.log(SwitchTest(undefined));
