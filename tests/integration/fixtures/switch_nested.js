// @expected-stdout: 6

// Nested switch — inner switch breaks only inner, outer continues
function SwitchTest(value) {
    var result = 0;

    switch(value) {
        case 0:
            switch(value) {
                case 0:
                    result += 3;
                    break;
                default:
                    result += 32;
                    break;
            }
            result *= 2;
            break;
            result = 3;
        default:
            result += 32;
            break;
    }
    return result;
}

console.log(SwitchTest(0));
