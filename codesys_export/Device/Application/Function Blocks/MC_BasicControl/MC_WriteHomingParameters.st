FUNCTION_BLOCK MC_WriteHomingParameters EXTENDS BaseStateMachine
VAR_IN_OUT
	Axis 						: AXIS_REF_SM3;//指定軸
	stParameter					: structAxisControl_HomingProfile;//參數表
END_VAR

VAR_INPUT
END_VAR
VAR_OUTPUT
END_VAR
VAR CONSTANT
	//專用於寫入Homing參數的變數表
	//寫入順序:
	//1. Homing Acceleration
	//2. Speed during search for zero
	//3. Speed during search for switch
	//4. Homing method
	PARAMETER_SIZE : INT := 4;
	
	HOMING_PARAMETER_INDEXES : ARRAY [1..PARAMETER_SIZE] OF WORD :=
	[16#609A, 16#6099, 16#6099, 16#6098];
	
	HOMING_PARAMETER_SUBINDEXES : ARRAY [1..PARAMETER_SIZE] OF WORD :=
	[16#00, 16#02, 16#01, 16#00];
	
	HOMING_PARAMETER_SIZES : ARRAY [1..PARAMETER_SIZE] OF INT :=
	[4, 4, 4, 1];
END_VAR
VAR
	iWriteParameterIndex : INT;
	lrAryWriteParameterStack 	: ARRAY [1..PARAMETER_SIZE] OF LREAL;//寫入的棧數據
	fbMC_WriteParameter		: MC_WriteParameter;
	diParameterNumber : DINT;
END_VAR

// === IMPLEMENTATION ===
SUPER^();