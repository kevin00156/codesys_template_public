METHOD PROTECTED Main 
VAR
	i : INT;
END_VAR

// === IMPLEMENTATION ===
CASE THIS^.iStep OF
	0://Init
	xBusy:=0;
	xError:=0;
	iErrorID:=0;
	fbMC_WriteParameter.Execute:=0;
	iWriteParameterIndex:=0;
	IF xExecute THEN
		iStep:=iStep+1;
	END_IF
	1:
	IF NOT fbMC_WriteParameter.Busy THEN
		iStep:=iStep+1;
		xBusy:=1;
	END_IF
	2:
	//處理數據
	lrAryWriteParameterStack[1]:=
		stParameter.lrHoming_Acceleration*
		stParameter.lrUnitToPulseRatio;
	lrAryWriteParameterStack[2]:=
		stParameter.lrVelocityDuringSearchForZero*
		stParameter.lrUnitToPulseRatio;
	lrAryWriteParameterStack[3]:=
		stParameter.lrVelocityDuringSearchForSwitch*
		stParameter.lrUnitToPulseRatio;
	lrAryWriteParameterStack[4]:=
		stParameter.uiHomingMethod;
	FOR i:=1 TO PARAMETER_SIZE DO
		IF lrAryWriteParameterStack[i]=SM3_Math.SMC_Math_Global.NaN THEN
			iStep:=9999;
			xError:=1;
		END_IF
	END_FOR
	iWriteParameterIndex:=1;
	iStep:=iStep+1;
	3:
	//寫入數據
	//這是說明書有關MC_WriteParameter上的公式，別動
	diParameterNumber :=
	- DWORD_TO_DINT(
		SHL(TO_DWORD(HOMING_PARAMETER_SIZES[iWriteParameterIndex]), 24)+
		SHL(TO_DWORD(HOMING_PARAMETER_INDEXES[iWriteParameterIndex]), 8)+
		HOMING_PARAMETER_SUBINDEXES[iWriteParameterIndex]
		);
	//指定參數
	fbMC_WriteParameter.ParameterNumber:=diParameterNumber;
	fbMC_WriteParameter.Value:=lrAryWriteParameterStack[iWriteParameterIndex];
	fbMC_WriteParameter.Execute:=1;
	iStep:=iStep+1;
	4:
	IF fbMC_WriteParameter.Done THEN//寫入完成
		fbMC_WriteParameter.Execute:=0;
		iWriteParameterIndex:=iWriteParameterIndex+1;
		IF iWriteParameterIndex>PARAMETER_SIZE THEN//若參數全部寫入完成則退出
			iStep:=100;
		ELSE//參數還沒全部寫入完成，則退到上一步繼續寫入
			iStep:=iStep-1;
		END_IF
	END_IF
	IF fbMC_WriteParameter.Error THEN
	 xError:=TRUE;
	 iErrorID:=fbMC_WriteParameter.ErrorID;
	 iStep:=9999;
	END_IF
ELSE
	xDone:=1;
	IF NOT xExecute THEN
		xDone:=0;
		iStep:=0;
	END_IF
END_CASE
fbMC_WriteParameter(Axis:=Axis);